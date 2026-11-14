#!/usr/bin/env bash
# The native import reader against the project's own TypeScript, on every
# script file under a folder: the oracle runs in a container, the native
# binary and the comparison run here, on data only.
#   native/oracle/run-imports.sh <dir> <out-dir>
# MODULES=<a node_modules with typescript> when the folder has none.
set -euo pipefail
directory=$(realpath "$1"); out=$(realpath -m "$2")
[ -d "$directory" ] || { echo "no such folder: $directory" >&2; exit 1; }
tool=$(realpath "$(dirname "$0")/../..")
bun_binary=${BUN_BINARY:-$(command -v bun)}
mkdir -p "$out"
modules=()
if [ -n "${MODULES:-}" ]; then
	modules=(-v "$(realpath "$MODULES")":/modules/node_modules:ro -e TYPESCRIPT_FROM=/modules)
fi
docker run --rm --network none --memory 6g --user "$(id -u):$(id -g)" -e HOME=/tmp \
	-v "$directory":/project:ro "${modules[@]}" -v "$tool":/tool:ro -v "$out":/out \
	-v "$bun_binary":/usr/local/bin/bun:ro -w /project node:22-bookworm \
	bun /tool/native/oracle/dump-imports.ts . /out ${MAX_MB:+--max-mb=$MAX_MB}
"$tool/native/target/release/vue-check-codegen" imports < "$out/files.jsonl" > "$out/native.jsonl"
bun "$tool/native/oracle/compare-imports.ts" "$out"
