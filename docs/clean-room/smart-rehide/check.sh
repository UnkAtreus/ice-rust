#!/bin/sh
# Clean-room gate for smart rehide. The rebuild must pass the spec tests in
# tests/rehide_spec.rs, and every intentionally broken version of
# src/rehide_policy.rs in broken/ must compile and then fail them. A broken
# version that does not compile proves nothing, so it fails the gate too.
#
# Usage: docs/clean-room/smart-rehide/check.sh   (exit 0 only if all hold)
#        docs/clean-room/smart-rehide/check.sh --compile-only
#        (only checks that each broken version compiles; runs no tests)
set -u

root=$(cd "$(dirname "$0")/../../.." && pwd)
broken_dir="$root/docs/clean-room/smart-rehide/broken"
target="$root/target/clean-room"
test_name=rehide_spec
status=0

cd "$root" || exit 1

if [ "${1:-}" = "--compile-only" ]; then
  work=$(mktemp -d)
  trap 'rm -rf "$work"' EXIT
  for broken in "$broken_dir"/*.rs; do
    [ -e "$broken" ] || continue
    name=$(basename "$broken" .rs)
    copy="$work/$name"
    mkdir -p "$copy"
    cp -R Cargo.toml Cargo.lock src assets "$copy/"
    cp "$broken" "$copy/src/rehide_policy.rs"
    if (cd "$copy" && CARGO_TARGET_DIR="$target" cargo check --quiet --lib --bins); then
      echo "COMPILES $name"
    else
      echo "INVALID $name: does not compile"
      status=1
    fi
  done
  exit "$status"
fi

echo "== rebuild"
if cargo test --quiet --test "$test_name"; then
  echo "PASS rebuild"
else
  echo "FAIL rebuild: the spec tests fail on src/rehide_policy.rs"
  status=1
fi

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

count=0
for broken in "$broken_dir"/*.rs; do
  [ -e "$broken" ] || continue
  count=$((count + 1))
  name=$(basename "$broken" .rs)
  copy="$work/$name"
  mkdir -p "$copy"
  cp -R Cargo.toml Cargo.lock src tests assets "$copy/"
  cp "$broken" "$copy/src/rehide_policy.rs"
  echo "== $name"
  if ! (cd "$copy" && CARGO_TARGET_DIR="$target" cargo test --quiet --test "$test_name" --no-run); then
    echo "INVALID $name: does not compile"
    status=1
  elif (cd "$copy" && CARGO_TARGET_DIR="$target" cargo test --quiet --test "$test_name"); then
    echo "SURVIVED $name: the spec tests pass on a broken version"
    status=1
  else
    echo "CAUGHT $name"
  fi
done

if [ "$count" -lt 3 ]; then
  echo "FAIL: expected 3 broken versions in $broken_dir, found $count"
  status=1
fi

[ "$status" -eq 0 ] && echo "== gate passed" || echo "== gate failed"
exit "$status"
