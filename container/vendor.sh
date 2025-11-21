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

  log "Verifying checksum for $archive_path..."
  # Copy checksum to /tmp for verification
  cp "$checksum_file" "/tmp/${archive_name}.sha256"
  if (cd /tmp && sha256sum -c "${archive_name}.sha256"); then
    log "Checksum verified for $archive_path"
  else
    log "ERROR: Checksum verification failed for $archive_path"
    rm -f "$archive_path" "/tmp/${archive_name}.sha256"
    exit 1
  fi

  log "Extracting $archive_path..."
  # Extract to a unique directory based on archive name
  local extract_dir="/tmp/extracted-$(basename ${archive_name} .tar.gz)"
  mkdir -p "$extract_dir"
  tar -xzf "$archive_path" -C "$extract_dir"
  rm -f "$archive_path" "/tmp/${archive_name}.sha256"
  touch "$extract_marker"
  log "Extracted and verified: $archive_name to $extract_dir"
}

# Call vendor with command-line arguments
vendor "$@"
