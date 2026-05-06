#!/usr/bin/env bash
set -euo pipefail

tag_name="${1:-${GITHUB_REF_NAME:-}}"
root_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
release_dir="$root_dir/target/release-artifacts"
appcast_dir="$root_dir/target/sparkle/${tag_name}"

if [[ -z "$tag_name" ]]; then
  echo "release tag is required" >&2
  exit 1
fi

if [[ ! "$tag_name" =~ ^v[0-9].* ]]; then
  echo "release tag must start with 'v' (for example: v0.1.0)" >&2
  exit 1
fi

if [[ -z "${GITHUB_TOKEN:-}" ]]; then
  echo "GITHUB_TOKEN is required" >&2
  exit 1
fi

if [[ -z "${CODESIGN_IDENTITY:-}" ]]; then
  echo "CODESIGN_IDENTITY is required" >&2
  exit 1
fi

if [[ -z "${SPARKLE_PRIVATE_ED_KEY:-}" ]]; then
  echo "SPARKLE_PRIVATE_ED_KEY GitHub repository secret is required" >&2
  exit 1
fi

mkdir -p "$release_dir"
rm -rf "$appcast_dir"

mise trust -y mise.toml
mise install
mise run verify

SPARKLE_ARCHIVES_DIR="$appcast_dir" \
SPARKLE_DOWNLOAD_URL_PREFIX="https://github.com/stephanos/nitpick-agent/releases/download/${tag_name}/" \
mise run macos-appcast

archive_path="$(find "$appcast_dir" -maxdepth 1 -name 'Nitpick-Agent-*.zip' -type f -print -quit)"
appcast_path="$appcast_dir/appcast.xml"

if [[ -z "$archive_path" ]]; then
  echo "release archive was not generated" >&2
  exit 1
fi

if [[ ! -f "$appcast_path" ]]; then
  echo "appcast was not generated" >&2
  exit 1
fi

archive_name="$(basename "$archive_path")"
release_archive_path="$release_dir/$archive_name"
release_checksum_path="$release_archive_path.sha256"
release_appcast_path="$release_dir/appcast.xml"

cp "$archive_path" "$release_archive_path"
cp "$appcast_path" "$release_appcast_path"
shasum -a 256 "$release_archive_path" > "$release_checksum_path"

gh release create "$tag_name" \
  "$release_archive_path" \
  "$release_checksum_path" \
  "$release_appcast_path" \
  --generate-notes
