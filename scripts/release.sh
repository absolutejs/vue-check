#!/usr/bin/env bash
# Publishes a vue-check release. CI (release.yml) builds the native codegen for
# every platform into a draft for the tag; this script, on the release machine,
# checks each binary, publishes one npm package per platform holding it, then
# @absolutejs/vue-check, which names them as optional dependencies: npm
# installs the one for the machine, and src/native.ts finds it. On a platform
# with no binary, vue-check runs the JavaScript codegen.
#
#   scripts/release.sh          # releases the version in package.json
set -euo pipefail
cd "$(dirname "$0")/.."
REPO=absolutejs/vue-check
VERSION=$(sed -n 's/^\t"version": "\(.*\)",/\1/p' package.json | head -1)
CRATE=$(sed -n 's/^version = "\(.*\)"/\1/p' native/Cargo.toml | head -1)
TAG="v$VERSION"
PLATFORMS="linux-x64 linux-arm64 darwin-arm64 darwin-x64 windows-x64 windows-arm64"
die() { echo "release: $*" >&2; exit 1; }
asset_of() { case "$1" in windows-*) echo "vue-check-codegen-$1.exe" ;; *) echo "vue-check-codegen-$1" ;; esac; }

[ -n "$VERSION" ] || die "no version in package.json"
[ "$VERSION" = "$CRATE" ] || die "package.json is $VERSION, native/Cargo.toml is $CRATE"
[ -z "$(git status --porcelain)" ] || die "the working tree is not clean"
[ "$(git branch --show-current)" = main ] || die "release from main"
git fetch -q origin
[ "$(git rev-parse HEAD)" = "$(git rev-parse origin/main)" ] || die "main is not pushed (or is behind origin)"
if gh release view "$TAG" -R "$REPO" --json isDraft -q .isDraft 2>/dev/null | grep -q false; then
  die "$TAG is already published"
fi

if ! git rev-parse -q --verify "refs/tags/$TAG" >/dev/null; then
  git tag -a "$TAG" -m "vue-check $VERSION"
  git push -q origin "$TAG"
  echo "release: tagged $TAG; waiting for the release build"
fi

# Wait for the build of exactly this tag to finish.
sha=$(git rev-list -n1 "$TAG")
run=""
for _ in $(seq 1 60); do
  run=$(gh run list -R "$REPO" --workflow release.yml --json databaseId,headSha -q "[.[] | select(.headSha==\"$sha\")][0].databaseId // empty")
  [ -n "$run" ] && break
  sleep 10
done
[ -n "$run" ] || die "no release build started for $TAG"
gh run watch "$run" -R "$REPO" --exit-status >/dev/null || die "the release build failed: gh run view $run -R $REPO"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
gh release download "$TAG" -R "$REPO" --dir "$work" --pattern 'vue-check-codegen-*'
for platform in $PLATFORMS; do [ -s "$work/$(asset_of "$platform")" ] || die "the draft is missing $(asset_of "$platform")"; done
[ "$(ls "$work" | wc -l)" = 6 ] || die "the draft has unexpected assets: $(ls "$work")"

# Each binary is the format and architecture its name claims.
check() { file -b "$work/$1" | grep -qE "$2" || die "$1 is not $2: $(file -b "$work/$1")"; }
check vue-check-codegen-linux-x64 'ELF 64-bit.*x86-64.*static'
check vue-check-codegen-linux-arm64 'ELF 64-bit.*aarch64.*static'
check vue-check-codegen-darwin-arm64 'Mach-O 64-bit.*arm64'
check vue-check-codegen-darwin-x64 'Mach-O 64-bit.*x86_64'
check vue-check-codegen-windows-x64.exe 'PE32\+.*x86-64'
check vue-check-codegen-windows-arm64.exe 'PE32\+.*Aarch64'
chmod +x "$work/vue-check-codegen-linux-x64"
got=$("$work/vue-check-codegen-linux-x64" --version)
[ "$got" = "vue-check-codegen $VERSION" ] || die "vue-check-codegen-linux-x64 reports '$got', not 'vue-check-codegen $VERSION'"

npm_dir="$work/npm"
optional=""
npm_names=""
# Whether the registry serves this exact version (so a rerun skips it).
published() { [ "$(npm view "$1@$VERSION" version 2>/dev/null)" = "$VERSION" ]; }
for platform in $PLATFORMS; do
  os=${platform%-*}; cpu=${platform#*-}
  npm_os=$os; [ "$os" = windows ] && npm_os=win32
  name="@absolutejs/vue-check-$platform"
  dir="$npm_dir/$platform"
  mkdir -p "$dir/bin"
  binary=vue-check-codegen; [ "$os" = windows ] && binary=vue-check-codegen.exe
  cp "$work/$(asset_of "$platform")" "$dir/bin/$binary"
  chmod +x "$dir/bin/$binary"
  cp LICENSE "$dir/"
  cat > "$dir/package.json" <<JSON
{
  "name": "$name",
  "version": "$VERSION",
  "description": "vue-check's native codegen for $platform (see @absolutejs/vue-check)",
  "license": "MIT",
  "repository": { "type": "git", "url": "git+https://github.com/absolutejs/vue-check.git" },
  "os": ["$npm_os"],
  "cpu": ["$cpu"],
  "files": ["bin", "LICENSE"]
}
JSON
  published "$name" || (cd "$dir" && npm publish --access public)
  optional="$optional\"$name\": \"$VERSION\","
  npm_names="$npm_names $name"
done

# @absolutejs/vue-check as the repository has it, plus the platform packages
# at exactly this version.
main="$npm_dir/vue-check"
mkdir -p "$main"
cp -R src README.md LICENSE package.json "$main/"
node -e '
const fs = require("node:fs");
const file = process.argv[1];
const pkg = JSON.parse(fs.readFileSync(file, "utf8"));
pkg.optionalDependencies = JSON.parse("{" + process.argv[2].replace(/,$/, "") + "}");
fs.writeFileSync(file, JSON.stringify(pkg, null, "\t") + "\n");
' "$main/package.json" "$optional"
published @absolutejs/vue-check || (cd "$main" && npm publish --access public)

# The version shows in the registry's metadata before its tarball downloads.
tarball_served() {
  url=$(npm view "$1@$VERSION" dist.tarball 2>/dev/null)
  [ -n "$url" ] && curl -fsI "$url" >/dev/null 2>&1
}
for _ in $(seq 1 90); do
  missing=""
  for name in $npm_names @absolutejs/vue-check; do
    { published "$name" && tarball_served "$name"; } || missing="$missing $name"
  done
  [ -z "$missing" ] && break
  sleep 20
done
[ -z "$missing" ] || die "npm is not serving$missing@$VERSION yet; rerun this script once it is"
echo "release: npm serves @absolutejs/vue-check@$VERSION and its six platform packages"

(cd "$work" && sha256sum vue-check-codegen-* > SHASUMS256.txt)
gh release upload "$TAG" -R "$REPO" --clobber "$work/SHASUMS256.txt"
gh release edit "$TAG" -R "$REPO" --draft=false --latest --notes "vue-check $VERSION. The binaries are the native codegen in the \`@absolutejs/vue-check-<platform>\` npm packages; \`SHASUMS256.txt\` lists their SHA-256."
echo "release: published https://github.com/$REPO/releases/tag/$TAG"
