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

# Verify <path> against <checksum_file> (a `sha256sum -c` file naming
# the bare file). Runs the check inside the file's own directory so the
# same helper serves /tmp downloads and vendor-cache entries alike.
verify_at() {
  local path=$1
  local checksum_file=$2
  local dir base
  dir=$(dirname "$path")
  base=$(basename "$path")
  cp "$checksum_file" "${dir}/${base}.sha256"
  if (cd "$dir" && sha256sum -c "${base}.sha256" >/dev/null 2>&1); then
    rm -f "${dir}/${base}.sha256"
    return 0
  fi
  rm -f "${dir}/${base}.sha256"
  return 1
}

# Download <url> and verify it against <checksum_file>, echoing the
# verified file's path on stdout (logs go to stderr). When
# VENDOR_CACHE_DIR is set - a BuildKit cache mount shared between the
# hierophant and contemplant image builds - the download lands in the
# cache keyed by its version-prefixed name and is reused by any later
# build of EITHER image, surviving layer-cache invalidation entirely.
# A cached file that fails its checksum (partial download, upstream
# re-tag) is discarded and re-fetched once.
# Usage: fetch_verified <name> <checksum_file> <url> <version_prefix>
fetch_verified() {
  local name=$1
  local checksum_file=$2
  local url=$3
  local version_prefix=$4
  local dest

  if [ -n "${VENDOR_CACHE_DIR:-}" ]; then
    dest="${VENDOR_CACHE_DIR}/${version_prefix}${name}"
    mkdir -p "$(dirname "$dest")"
    if [ -f "$dest" ]; then
      if verify_at "$dest" "$checksum_file"; then
        log "Using cached $name from vendor cache" >&2
        echo "$dest"
        return 0
      fi
      log "Cached $name failed checksum; re-downloading" >&2
      rm -f "$dest"
    fi
  else
    dest="/tmp/${name}"
  fi

  log "Downloading: $url" >&2
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL -o "$dest" "$url"
  else
    log "ERROR: curl not found!" >&2
    exit 1
  fi

  if verify_at "$dest" "$checksum_file"; then
    log "Downloaded and verified: $dest" >&2
  else
    log "ERROR: Checksum verification failed for $dest" >&2
    rm -f "$dest"
    exit 1
  fi
  echo "$dest"
}

# Function to download, verify, and extract a compressed archive
# Usage: vendor <archive_name> <checksum_dir> <version_prefix>
vendor() {
  local archive_name=$1
  local checksum_dir=$2
  local version_prefix=$3
  local checksum_file="${checksum_dir}/${archive_name}.sha256"
  local url="${VENDOR_BASE_URL}/${version_prefix}${archive_name}"
  local extract_marker="/tmp/.extracted-${archive_name}"
  local archive_path

  # Check if already extracted and verified
  if [ -f "$extract_marker" ]; then
    log "Archive $archive_name already extracted"
    return 0
  fi

  archive_path=$(fetch_verified \
    "$archive_name" "$checksum_file" "$url" "$version_prefix")

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
  # A /tmp download is dead weight once extracted; a vendor-cache entry
  # is the whole point - keep it for the next build.
  if [ -z "${VENDOR_CACHE_DIR:-}" ]; then
    rm -f "$archive_path"
  fi
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
  local fetched

  if [ -f "$marker" ]; then
    log "File $file_name already vendored"
    return 0
  fi

  fetched=$(fetch_verified \
    "$file_name" "$checksum_file" "$url" "$version_prefix")

  # Call sites consume (and often `mv`) the file from /tmp; copy out of
  # the cache so the cached original survives for the next build.
  if [ "$fetched" != "$file_path" ]; then
    cp "$fetched" "$file_path"
  fi
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
