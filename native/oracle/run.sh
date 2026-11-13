#!/usr/bin/env bash
# The native parser against the project's own, on every .vue file of a
# project: the oracle runs in a container (it loads the project's packages),
# the native binary and the comparison run here, on data only.
#   native/oracle/run.sh <project-dir> <out-dir>
# For a package in a monorepo, set WORKSPACE=<monorepo root>; for a copy that
# borrows another checkout's node_modules, MODULES=<path> (as bench/run-in-docker.sh).
set -euo pipefail
project=$(realpath "$1"); out=$(realpath -m "$2")
tool=$(realpath "$(dirname "$0")/../..")
bun_binary=${BUN_BINARY:-$(command -v bun)}
mkdir -p "$out"
mount=$project; inside=/project
if [ -n "${WORKSPACE:-}" ]; then
	mount=$(realpath "$WORKSPACE"); inside=/project/$(realpath --relative-to="$mount" "$project")
fi
modules=()
if [ -n "${MODULES:-}" ]; then
	modules=(-v "$(realpath "$MODULES")":/project/node_modules:ro)
fi
docker run --rm --network none --memory 4g --user "$(id -u):$(id -g)" -e HOME=/tmp \
	-v "$mount":/project:ro "${modules[@]}" -v "$tool":/tool:ro -v "$out":/out \
	-v "$bun_binary":/usr/local/bin/bun:ro -w "$inside" node:22-bookworm \
	bun /tool/native/oracle/dump.ts "$inside" /out
"$tool/native/target/release/vue-check-codegen" parse < "$out/files.jsonl" > "$out/native.jsonl"
bun "$tool/native/oracle/compare.ts" "$out"
