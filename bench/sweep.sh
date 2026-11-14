#!/usr/bin/env bash
# The whole proof over a corpus built by bench/setup-corpus.sh: every project
# in bench/corpus.txt, the planted-error copy, and Vue's conformance suite,
# each through the four-way differential. One summary line per project.
#   bench/sweep.sh <corpus-dir> [--runs=5]
set -uo pipefail
dir=$(realpath "$1"); shift; tool=$(realpath "$(dirname "$0")/..")
runs=1; for arg in "$@"; do case "$arg" in --runs=*) runs=${arg#--runs=} ;; esac; done
cd "$tool"
summary() { jq -c '{identical, bunCheckMatchesTsc7, oraclesFinished, vueTsc: .summary["vue-tsc"].errors, handling: (.vueHandling|map_values(length)), e2e: (.endToEnd|map_values(length)), s: [.summary["vue-tsc"].medianWallS, .summary["vue-check"].medianWallS, .summary["vue-tsc"].medianPeakRssMB, .summary["vue-check"].medianPeakRssMB]}' "bench/results/$1.json"; }
grep -v '^#' bench/corpus.txt | while IFS='|' read -r name _repository _commit project workspace tsconfig args; do
	root=${project%%/*}
	if [ "$workspace" = "-" ]; then env=(); else env=(WORKSPACE="$dir/$workspace"); fi
	# A run that failed leaves the previous result file: never report that as this run's.
	if env "${env[@]}" timeout 1500 bench/run-in-docker.sh "$dir/$project" "$name" --runs="$runs" --tsconfig="$tsconfig" --vue-tsc-args="$args" >/dev/null 2>&1; then
		echo "$name $(summary "$name")"
	else
		echo "$name RUN FAILED"
	fi
done
if MODULES="$dir/vue-pure-admin/node_modules" timeout 1500 bench/run-in-docker.sh "$dir/pure-admin-planted" vue-pure-admin-planted-errors --runs=1 --vue-tsc-args=--skipLibCheck >/dev/null 2>&1; then
	echo "planted $(summary vue-pure-admin-planted-errors)"
else
	echo "planted RUN FAILED"
fi
docker run --rm --network none --memory 10g --user "$(id -u):$(id -g)" -e HOME=/tmp -e VUE_CHECK_VERIFY_SPANS="${VUE_CHECK_VERIFY_SPANS:-}" -v "$dir/conformance":/project -v "$tool":/tool:ro \
	-v "$tool/bench/results":/tool/bench/results -v "${BUN_BINARY:-$(command -v bun)}":/usr/local/bin/bun:ro -w /project node:22-bookworm \
	bun /tool/bench/conformance.ts /project --jobs=4 >/dev/null 2>&1 || { echo "conformance RUN FAILED"; exit 1; }
echo "conformance $(jq -c '{cases, identical, bunMatchesTsc7, failing: [.failing[].name]}' bench/results/conformance.json)"
