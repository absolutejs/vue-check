#!/usr/bin/env bash
# The bar for the native codegen (docs/NATIVE.md), on one project: the
# JavaScript codegen and the native one each write the project's output
# folder, and bench/codegen-diff.ts compares the two. In a container, as every
# third-party project runs: no network, the project and this tool read-only.
#   bench/codegen-bar.sh <project-dir> <tsconfig, relative to it> <out-dir>
# WORKSPACE and MODULES as bench/run-in-docker.sh.
set -euo pipefail
project=$(realpath "$1"); tsconfig=$2; out=$(realpath -m "$3")
[ -d "$project" ] || { echo "no such project: $project" >&2; exit 1; }
tool=$(realpath "$(dirname "$0")/..")
bun_binary=${BUN_BINARY:-$(command -v bun)}
rm -rf "$out"; mkdir -p "$out"
mount=$project; inside=/project
if [ -n "${WORKSPACE:-}" ]; then
	mount=$(realpath "$WORKSPACE"); inside=/project/$(realpath --relative-to="$mount" "$project")
fi
modules=()
if [ -n "${MODULES:-}" ]; then
	modules=(-v "$(realpath "$MODULES")":/project/node_modules:ro)
fi
# Both runs write the same folder (its path is part of the output), which is
# then moved aside.
docker run --rm --network none --memory "${MEMORY:-10g}" --user "$(id -u):$(id -g)" -e HOME=/tmp \
	-v "$mount":/project:ro "${modules[@]}" -v "$tool":/tool:ro -v "$out":/out \
	-v "$bun_binary":/usr/local/bin/bun:ro -w "$inside" node:22-bookworm bash -c '
	set -euo pipefail
	run() {
		local started=$(date +%s%N)
		VUE_CHECK_NATIVE=$1 VUE_CHECK_NATIVE_EXPLAIN=1 bun /tool/src/codegen.ts "$3" "$3/$4" /out/vue-check
		echo "$2 codegen: $(( ($(date +%s%N) - started) / 1000000 )) ms"
		mv /out/vue-check "/out/$2"
	}
	run 0 javascript "$0" "$1"
	run "" native "$0" "$1"
	bun /tool/bench/codegen-diff.ts /out/javascript /out/native
' "$inside" "$tsconfig"
