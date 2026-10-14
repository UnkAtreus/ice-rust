#!/usr/bin/env bash
# Builds and signs a release of ice-rust into dist/ and prints the command
# that publishes it. Publishing is left to the person running the script.
#
#   scripts/release.sh <version>
#   OUT_DIR=<dir> scripts/release.sh <version>   build into <dir> instead,
#                                  for example while dist/IceRust.app runs
#
# Needs: the version already set in Cargo.toml, the private update key at
# $ICE_RUST_UPDATE_KEY (default ~/.config/ice-rust/update-signing.key), and
# a build whose compiled-in public key matches it. A build that still has
# the placeholder key fails at the verify step.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

version="${1:-}"
if [[ ! "$version" =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]]; then
    echo "usage: scripts/release.sh MAJOR.MINOR.PATCH" >&2
    exit 2
fi
cargo_version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)"
if [[ "$cargo_version" != "$version" ]]; then
    echo "Cargo.toml has version $cargo_version, not $version; bump it first" >&2
    exit 1
fi
key="${ICE_RUST_UPDATE_KEY:-$HOME/.config/ice-rust/update-signing.key}"
if [[ ! -f "$key" ]]; then
    echo "no private update key at $key" >&2
    exit 1
fi

mkdir -p "${OUT_DIR:-dist}"
out="$(cd "${OUT_DIR:-dist}" && pwd)"
OUT_DIR="$out" scripts/bundle.sh
bin="$out/IceRust.app/Contents/MacOS/ice-rust"
name="IceRust-$version-arm64"
cd "$out"
rm -rf "$name" "$name.zip" "$name.zip.sha256" "$name.zip.sig" "$name-src"
# The zip holds IceRust.app alone, as the updater requires; LICENSE and
# NOTICE travel inside the bundle (Contents/Resources, see bundle.sh).
# Extended attributes and resource forks stay out, so no __MACOSX folder.
ditto -c -k --norsrc --noextattr --noacl --keepParent IceRust.app "$name.zip"
"$bin" update-sign --key "$key" "$name.zip"
"$bin" update-verify "$name.zip"
shasum -a 256 "$name.zip" > "$name.zip.sha256"

cat <<CMD

Built and signed $out/$name.zip. Check it, then publish with:

  git tag -a v$version -m "ice-rust $version" && git push origin v$version
  gh release create v$version "$out/$name.zip" "$out/$name.zip.sig" "$out/$name.zip.sha256" \\
      --title "ice-rust $version" --notes-file <notes.md>
CMD
