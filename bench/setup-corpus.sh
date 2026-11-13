#!/usr/bin/env bash
# Builds the benchmark corpus in <dir>: every project in bench/corpus.txt at its
# commit, the planted-error copy of vue-pure-admin, and Vue's conformance
# workspace. Third-party dependencies are installed only inside a container,
# with install scripts off; nothing from these projects runs on the host.
#   bench/setup-corpus.sh <dir>
set -euo pipefail
dir=$(realpath -m "$1"); tool=$(realpath "$(dirname "$0")/..")
bun_binary=${BUN_BINARY:-$(command -v bun)}
mkdir -p "$dir"
install() { # <folder>
	docker run --rm --memory 6g --user "$(id -u):$(id -g)" -e HOME=/tmp -v "$1":/project \
		-v "$bun_binary":/usr/local/bin/bun:ro -w /project node:22-bookworm bun install --ignore-scripts >/dev/null
}
clone() { # <repository> <commit> <folder>
	[ -d "$3/.git" ] && return
	git init -q "$3" && git -C "$3" remote add origin "https://github.com/$1.git"
	git -C "$3" fetch -q --depth 1 origin "$2" && git -C "$3" checkout -q FETCH_HEAD
}
grep -v '^#' "$tool/bench/corpus.txt" | while IFS='|' read -r name repository commit project workspace _tsconfig _args; do
	root=${project%%/*}
	clone "$repository" "$commit" "$dir/$root"
	install "$dir/${workspace/#-/$root}"
	echo "ready: $name"
done
# 240 planted errors of 12 kinds across a copy of vue-pure-admin (borrows its node_modules).
rm -rf "$dir/pure-admin-planted"
rsync -a --exclude node_modules --exclude .vue-check --exclude .git "$dir/vue-pure-admin/" "$dir/pure-admin-planted/"
bun "$tool/bench/mutate.ts" "$dir/pure-admin-planted" --count=240 >/dev/null
echo "ready: vue-pure-admin-planted-errors"
# Vue's conformance cases (vuejs/language-tools test-workspace/tsc) with the published vue-tsc.
clone vuejs/language-tools 31e5f34 "$dir/language-tools"
mkdir -p "$dir/conformance" && rm -rf "$dir/conformance/tsc"
cp -r "$dir/language-tools/test-workspace/tsc" "$dir/language-tools/test-workspace/tsconfig.base.json" "$dir/conformance/"
printf '{\n\t"private": true,\n\t"devDependencies": {\n\t\t"typescript": "5.9.3",\n\t\t"vue": "3.6.0-rc.6",\n\t\t"vue-component-type-helpers": "3.3.14",\n\t\t"vue-tsc": "3.3.14"\n\t}\n}\n' > "$dir/conformance/package.json"
install "$dir/conformance"
echo "ready: conformance"
