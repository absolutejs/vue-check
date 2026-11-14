#!/usr/bin/env bash
# The native template-expression codegen against the original's, on generated
# expressions (dump-expressions.ts, in a container; the rest runs here).
#   native/oracle/run-expressions.sh <project-with-language-core-3.3.14> <out-dir> [--count=20000] [--seed=1]
set -euo pipefail
project=$(realpath "$1"); out=$(realpath -m "$2"); shift 2
[ -d "$project" ] || { echo "no such project: $project" >&2; exit 1; }
tool=$(realpath "$(dirname "$0")/../..")
bun_binary=${BUN_BINARY:-$(command -v bun)}
mkdir -p "$out"
docker run --rm --network none --memory 4g --user "$(id -u):$(id -g)" -e HOME=/tmp \
	-v "$project":/project:ro -v "$tool":/tool:ro -v "$out":/out \
	-v "$bun_binary":/usr/local/bin/bun:ro -w /project node:22-bookworm \
	bun /tool/native/oracle/dump-expressions.ts /project /out "$@"
"$tool/native/target/release/vue-check-codegen" interpolate < "$out/oracle.jsonl" > "$out/interpolations.jsonl"
bun "$tool/native/oracle/compare-interpolations.ts" "$out"
