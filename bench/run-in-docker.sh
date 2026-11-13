#!/usr/bin/env bash
# Runs bench/run.ts against a third-party project in an isolated container:
# no network, a memory cap, this tool mounted read-only. The project folder is
# mounted writable only so vue-check can write its .vue-check/ beside it.
#   bench/run-in-docker.sh <project-dir> <name> [extra args for run.ts]
set -euo pipefail
project=$(realpath "$1"); name=$2; shift 2
tool=$(realpath "$(dirname "$0")/..")
bun_binary=${BUN_BINARY:-$(command -v bun)}
mkdir -p "$tool/bench/results"
memory=${MEMORY:-10g}
# A project copy can borrow the original's node_modules (MODULES=<path>):
# mounted read-only over an empty node_modules folder in the copy.
modules=()
if [ -n "${MODULES:-}" ]; then
	[ -L "$project/node_modules" ] && rm "$project/node_modules"
	mkdir -p "$project/node_modules"
	modules=(-v "$(realpath "$MODULES")":/project/node_modules:ro)
fi
docker run --rm --network none --memory "$memory" --memory-swap "$memory" \
	--user "$(id -u):$(id -g)" -e HOME=/tmp \
	-v "$project":/project \
	"${modules[@]}" \
	-v "$tool":/tool:ro \
	-v "$tool/bench/results":/tool/bench/results \
	-v "$bun_binary":/usr/local/bin/bun:ro \
	-w /project node:22-bookworm \
	bun /tool/bench/run.ts /project --name="$name" "$@"
