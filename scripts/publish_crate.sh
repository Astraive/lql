#!/usr/bin/env bash
# Publish a crate, treating only an exact-version "already published" response
# as success. Any other publish failure fails the caller.
#
# Usage: publish_crate.sh <manifest-path> <crate-name> [registry]
set -euo pipefail

manifest="${1:?usage: publish_crate.sh <manifest-path> <crate-name> [registry]}"
crate="${2:?usage: publish_crate.sh <manifest-path> <crate-name> [registry]}"
registry="${3:-crates-io}"

# Single source of truth: the version declared by the crate's own manifest.
version="$(awk -F '"' '/^version =/{print $2; exit}' "$manifest")"
if [[ -z "$version" ]]; then
  echo "ERROR: could not read version from ${manifest}" >&2
  exit 1
fi

if cargo publish --manifest-path "$manifest"; then
  echo "${crate} ${version} published successfully"
  exit 0
fi

# Publish failed. Only succeed when the EXACT expected version is already in
# the registry. Checking an older/stale version here would hide real failures.
if cargo info --registry "$registry" "${crate}@${version}" >/dev/null 2>&1; then
  echo "${crate} ${version} is already published; continuing"
  exit 0
fi

echo "ERROR: ${crate} ${version} publish failed and the exact version is not present in ${registry}" >&2
exit 1
