#!/usr/bin/env sh
# vendor.sh - Download vendored dependencies
#
# This script downloads large binary dependencies that are too big for git.
# Checksums are verified to ensure integrity.
#
# Usage: source this script and call vendor() function
#   vendor <archive_name> <checksum_dir> <version_prefix>

set -euo pipefail

# Log messages with a consistent prefix.
log() {
  echo "[vendor] $*"
}

# Function to download, verify, and extract a compressed archive
# Usage: vendor <archive_name> <checksum_dir> <version_prefix>
vendor() {
  local archive_name=$1
  local checksum_dir=$2
  local version_prefix=$3
  local archive_path="/tmp/${archive_name}"
  local checksum_file="${checksum_dir}/${archive_name}.sha256"
  local url="${VENDOR_BASE_URL}/${version_prefix}${archive_name}"
  local extract_marker="/tmp/.extracted-${archive_name}"

  # Check if already extracted and verified
  if [ -f "$extract_marker" ]; then
    log "Archive $archive_name already extracted"
    return 0
  fi

  log "Downloading: $url"

  if command -v curl >/dev/null 2>&1; then
    curl -fsSL -o "$archive_path" "$url"
  else
    log "ERROR: curl not found!"
    exit 1
  fi

  # Copy checksum to /tmp for verification
  log "Verifying checksum for $archive_path..."
  cp "$checksum_file" "/tmp/${archive_name}.sha256"
  if (cd /tmp && sha256sum -c "${archive_name}.sha256"); then
    log "Checksum verified for $archive_path"
  else
    log "ERROR: Checksum verification failed for $archive_path"
    rm -f "$archive_path" "/tmp/${archive_name}.sha256"
    exit 1
  fi

  # Extract to a unique directory based on archive name. Strip both
  # `.tar.gz` and `.tar.xz` suffixes so the directory name is readable
  # regardless of compression (the rzup-shaped risc0-groth16 tarball ships
  # as .tar.xz while the SP1 / moongate / legacy-risc0 tarballs are .tar.gz).
  log "Extracting $archive_path..."
  local base
  base=$(basename "$archive_name")
  base=${base%.tar.gz}
  base=${base%.tar.xz}
  local extract_dir="/tmp/extracted-${base}"
  mkdir -p "$extract_dir"
  tar -xf "$archive_path" -C "$extract_dir"
  rm -f "$archive_path" "/tmp/${archive_name}.sha256"
  touch "$extract_marker"
  log "Extracted and verified: $archive_name to $extract_dir"
}

# Download and verify a raw (non-archive) vendored file, leaving it at
# /tmp/<file_name> with no extraction step. Same argument shape as vendor().
# Used for the OpenVM EVM assets (halo2.pk and the kzg_bn254_<k>.srs params),
# which are consumed as-is rather than unpacked.
# Usage: vendor_file <file_name> <checksum_dir> <version_prefix>
vendor_file() {
  local file_name=$1
  local checksum_dir=$2
  local version_prefix=$3
  local file_path="/tmp/${file_name}"
  local checksum_file="${checksum_dir}/${file_name}.sha256"
  local url="${VENDOR_BASE_URL}/${version_prefix}${file_name}"
  local marker="/tmp/.vendored-${file_name}"

  if [ -f "$marker" ]; then
    log "File $file_name already vendored"
    return 0
  fi

  log "Downloading: $url"
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL -o "$file_path" "$url"
  else
    log "ERROR: curl not found!"
    exit 1
  fi

  log "Verifying checksum for $file_path..."
  cp "$checksum_file" "/tmp/${file_name}.sha256"
  if (cd /tmp && sha256sum -c "${file_name}.sha256"); then
    log "Downloaded and verified: $file_path"
  else
    log "ERROR: Checksum verification failed for $file_path"
    rm -f "$file_path" "/tmp/${file_name}.sha256"
    exit 1
  fi
  rm -f "/tmp/${file_name}.sha256"
  touch "$marker"
}

# Dispatch: `vendor.sh --file <args>` fetches a raw file; the default
# (archive) mode stays argument-compatible with every existing call site.
if [ "${1:-}" = "--file" ]; then
  shift
  vendor_file "$@"
else
  vendor "$@"
fi
