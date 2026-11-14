#!/usr/bin/env bash
# The native parser against the project's own, on every .vue file of a
# project: the oracle runs in a container (it loads the project's packages),
# the native binary and the comparison run here, on data only.
#   native/oracle/run.sh <project-dir> <out-dir>
# For a package in a monorepo, set WORKSPACE=<monorepo root>; for a copy that
# borrows another checkout's node_modules, MODULES=<path> (as bench/run-in-docker.sh).
# TARGET=<vue version> generates for that version of Vue (default: the latest).
# VUE_OPTIONS=<json> generates with those vueCompilerOptions (default: none set).
set -euo pipefail
project=$(realpath "$1"); out=$(realpath -m "$2")
# Docker would create a missing folder itself, owned by root.
[ -d "$project" ] || { echo "no such project: $project" >&2; exit 1; }
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
docker run --rm --network none --memory 4g --user "$(id -u):$(id -g)" -e HOME=/tmp -e TARGET="${TARGET:-}" -e VUE_OPTIONS="${VUE_OPTIONS:-}" \
	-v "$mount":/project:ro "${modules[@]}" -v "$tool":/tool:ro -v "$out":/out \
	-v "$bun_binary":/usr/local/bin/bun:ro -w "$inside" node:22-bookworm \
	bun /tool/native/oracle/dump.ts "$inside" /out
core=--core=$(cat "$out/core.txt"); dom=--dom=$(cat "$out/dom.txt")
"$tool/native/target/release/vue-check-codegen" parse "$core" "$dom" < "$out/files.jsonl" > "$out/native.jsonl"
status=0
bun "$tool/native/oracle/compare.ts" "$out" || status=1
# The template expressions the oracle recorded, replayed natively.
"$tool/native/target/release/vue-check-codegen" interpolate "$core" "$dom" < "$out/oracle.jsonl" > "$out/interpolations.jsonl"
bun "$tool/native/oracle/compare-interpolations.ts" "$out" || status=1
# The templates the oracle recorded, generated natively.
"$tool/native/target/release/vue-check-codegen" template "$core" "$dom" < "$out/oracle.jsonl" > "$out/templates.jsonl"
bun "$tool/native/oracle/compare-templates.ts" "$out" || status=1
# Each file's generated code and mappings.
"$tool/native/target/release/vue-check-codegen" generate < "$out/files.jsonl" > "$out/generated.jsonl"
bun "$tool/native/oracle/compare-generated.ts" "$out" || status=1
exit $status
