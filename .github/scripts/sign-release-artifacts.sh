#!/bin/sh
# Sign release artifacts with GPG for multiple images.
#
# This script imports a GPG private key and signs the image digest manifests
# for all images in the release. Images are discovered dynamically from
# digest-outputs directory.
#
# Environment variables required:
#   - GPG_PRIVATE_KEY: Base64-encoded GPG private key
#   - GPG_PASSPHRASE: Passphrase for the GPG private key
#   - BUILD_TIMESTAMP: Build timestamp for release identification
#   - BUILD_SHA_SHORT: Short git commit SHA
#   - GITHUB_SHA: Full git commit SHA
#   - GITHUB_REPOSITORY: Repository name (owner/repo)
#   - GITHUB_REPOSITORY_OWNER: Repository owner
#   - DO_REGISTRY_NAME: DigitalOcean registry name
#   - DH_USERNAME: Docker Hub username
#   - <image>_DO_DIGEST, <image>_GHCR_DIGEST, <image>_DH_DIGEST: Per-image digests

set -e

echo "Setting up GPG signing for multi-image release ..."

# Set up GPG home directory in a writable location
export GNUPGHOME="${GITHUB_WORKSPACE}/.gnupg"
mkdir -p "$GNUPGHOME"
chmod 700 "$GNUPGHOME"

# Verify required secrets are present
if [ -z "$GPG_PRIVATE_KEY" ]; then
  echo "❌ GPG_PRIVATE_KEY is not set. GPG signing is mandatory."
  exit 1
fi

if [ -z "$GPG_PASSPHRASE" ]; then
  echo "❌ GPG_PASSPHRASE is not set. GPG signing is mandatory."
  exit 1
fi

# Import GPG private key (assuming base64 encoded)
echo "Importing GPG private key..."
if ! echo "$GPG_PRIVATE_KEY" | base64 -d | gpg --batch --quiet --import 2>&1; then
  echo "❌ Failed to import GPG private key"
  exit 1
fi
echo "✅ GPG private key imported successfully"

# Get the key ID
KEY_ID=$(gpg --list-secret-keys --keyid-format LONG | \
  grep sec | awk '{print $2}' | cut -d'/' -f2 | head -1)
echo "Using GPG key ID: ${KEY_ID: -16}"

# Create artifacts directory
mkdir -p release-artifacts

# Discover all images dynamically from digest-outputs directory
if [ ! -d "digest-outputs" ]; then
  echo "❌ digest-outputs directory not found"
  exit 1
fi

# Create combined digest manifest file header
cat > release-artifacts/image-digests.txt <<EOF
Multi-Image Release Digests
Release: $BUILD_TIMESTAMP-$BUILD_SHA_SHORT
Git SHA: $GITHUB_SHA
Build Date: $(date -u +"%Y-%m-%d %H:%M:%S UTC")

EOF

# Dynamically add each image to the digest manifest
for file in digest-outputs/*-digests.env; do
  if [ -f "$file" ]; then
    # Get matrix image name (internal identifier)
    MATRIX_IMAGE=$(basename "$file" | sed 's/-digests.env$//')
    MATRIX_IMAGE_UPPER=$(echo "$MATRIX_IMAGE" | tr '[:lower:]' '[:upper:]')

    # Source the digest file
    . "$file"

    # Get digest variables for this matrix image
    eval DO_DIGEST=\$${MATRIX_IMAGE}_DO_DIGEST
    eval GHCR_DIGEST=\$${MATRIX_IMAGE}_GHCR_DIGEST
    eval DH_DIGEST=\$${MATRIX_IMAGE}_DH_DIGEST
    eval PUBLISHED_NAME=\$${MATRIX_IMAGE}_PUBLISHED_NAME

    # Append to manifest using published name
    cat >> release-artifacts/image-digests.txt <<EOF
${MATRIX_IMAGE_UPPER} IMAGE (published as ${PUBLISHED_NAME}):
  GHCR:  ghcr.io/$GITHUB_REPOSITORY_OWNER/${PUBLISHED_NAME}@$GHCR_DIGEST
  DHCR:  $DH_USERNAME/${PUBLISHED_NAME}@$DH_DIGEST
  DOCR:  registry.digitalocean.com/$DO_REGISTRY_NAME/${PUBLISHED_NAME}@$DO_DIGEST

EOF
  fi
done

# Sign the digest manifest
echo "Signing image-digests.txt ..."
if gpg --batch --yes --pinentry-mode loopback \
    --passphrase "$GPG_PASSPHRASE" \
    --armor --detach-sign \
    --local-user "$KEY_ID" \
    release-artifacts/image-digests.txt 2>/dev/null; then

  if [ -f release-artifacts/image-digests.txt.asc ]; then
    echo "✅ Created signature: image-digests.txt.asc"
  else
    echo "❌ Failed to create signature file"
    exit 1
  fi
else
  echo "❌ GPG signing failed for image-digests.txt"
  exit 1
fi

# Verify the signature
if gpg --verify release-artifacts/image-digests.txt.asc \
    release-artifacts/image-digests.txt 2>&1 | \
    grep -q "Good signature"; then
  echo "✅ Signature verified successfully"
else
  echo "❌ Signature verification failed"
  exit 1
fi

# Sign image manifests from registries for each image
echo ""
echo "Fetching and signing image manifests ..."

# Dynamically process all images
for file in digest-outputs/*-digests.env; do
  if [ -f "$file" ]; then
    # Get matrix image name (for file naming)
    MATRIX_IMAGE=$(basename "$file" | sed 's/-digests.env$//')
    MATRIX_IMAGE_UPPER=$(echo "$MATRIX_IMAGE" | tr '[:lower:]' '[:upper:]')

    echo ""
    echo "Processing $MATRIX_IMAGE ..."

    # Source the digest file
    . "$file"

    # Get digest variables and published name for this image
    eval DO_DIGEST=\$${MATRIX_IMAGE}_DO_DIGEST
    eval GHCR_DIGEST=\$${MATRIX_IMAGE}_GHCR_DIGEST
    eval DH_DIGEST=\$${MATRIX_IMAGE}_DH_DIGEST
    eval PUBLISHED_NAME=\$${MATRIX_IMAGE}_PUBLISHED_NAME

    echo "Published as: $PUBLISHED_NAME"

    # Sign GHCR manifest (fetch using published name, save using matrix name)
    echo "Fetching GHCR manifest for $PUBLISHED_NAME ..."
    if docker manifest inspect \
        "ghcr.io/$GITHUB_REPOSITORY/${PUBLISHED_NAME}@$GHCR_DIGEST" \
        --verbose > release-artifacts/${MATRIX_IMAGE}-ghcr-manifest.json 2>/dev/null; then

      echo "Signing ${MATRIX_IMAGE}-ghcr-manifest.json ..."
      if gpg --batch --yes --pinentry-mode loopback \
          --passphrase "$GPG_PASSPHRASE" \
          --armor --detach-sign \
          --local-user "$KEY_ID" \
          release-artifacts/${MATRIX_IMAGE}-ghcr-manifest.json 2>/dev/null; then
        echo "✅ Signed ${MATRIX_IMAGE}-ghcr-manifest.json"
      else
        echo "❌ Failed to sign ${MATRIX_IMAGE}-ghcr-manifest.json"
        exit 1
      fi
    else
      echo "❌ Failed to fetch GHCR manifest for $PUBLISHED_NAME"
      exit 1
    fi

    # Sign Docker Hub manifest
    echo "Fetching Docker Hub manifest for $PUBLISHED_NAME ..."
    if docker manifest inspect \
        "$DH_USERNAME/${PUBLISHED_NAME}@$DH_DIGEST" \
        --verbose > release-artifacts/${MATRIX_IMAGE}-dh-manifest.json 2>/dev/null; then

      echo "Signing ${MATRIX_IMAGE}-dh-manifest.json ..."
      if gpg --batch --yes --pinentry-mode loopback \
          --passphrase "$GPG_PASSPHRASE" \
          --armor --detach-sign \
          --local-user "$KEY_ID" \
          release-artifacts/${MATRIX_IMAGE}-dh-manifest.json 2>/dev/null; then
        echo "✅ Signed ${MATRIX_IMAGE}-dh-manifest.json"
      else
        echo "❌ Failed to sign ${MATRIX_IMAGE}-dh-manifest.json"
        exit 1
      fi
    else
      echo "❌ Failed to fetch Docker Hub manifest for $PUBLISHED_NAME"
      exit 1
    fi

    # Sign DigitalOcean manifest
    echo "Fetching DigitalOcean manifest for $PUBLISHED_NAME ..."
    if docker manifest inspect \
        "registry.digitalocean.com/$DO_REGISTRY_NAME/${PUBLISHED_NAME}@$DO_DIGEST" \
        --verbose > release-artifacts/${MATRIX_IMAGE}-do-manifest.json 2>/dev/null; then

      echo "Signing ${MATRIX_IMAGE}-do-manifest.json ..."
      if gpg --batch --yes --pinentry-mode loopback \
          --passphrase "$GPG_PASSPHRASE" \
          --armor --detach-sign \
          --local-user "$KEY_ID" \
          release-artifacts/${MATRIX_IMAGE}-do-manifest.json 2>/dev/null; then
        echo "✅ Signed ${MATRIX_IMAGE}-do-manifest.json"
      else
        echo "❌ Failed to sign ${MATRIX_IMAGE}-do-manifest.json"
        exit 1
      fi
    else
      echo "❌ Failed to fetch DigitalOcean manifest for $PUBLISHED_NAME"
      exit 1
    fi
  fi
done

# Verify all manifest signatures
echo ""
echo "Verifying manifest signatures ..."
ALL_VERIFIED=true

for file in digest-outputs/*-digests.env; do
  if [ -f "$file" ]; then
    IMAGE=$(basename "$file" | sed 's/-digests.env$//')
    for manifest in ghcr dh do; do
      if gpg --verify \
          "release-artifacts/${IMAGE}-${manifest}-manifest.json.asc" \
          "release-artifacts/${IMAGE}-${manifest}-manifest.json" 2>&1 | \
          grep -q "Good signature"; then
        echo "✅ ${IMAGE}-${manifest}-manifest.json signature verified"
      else
        echo "❌ ${IMAGE}-${manifest}-manifest.json signature verification failed"
        ALL_VERIFIED=false
      fi
    done
  fi
done

if [ "$ALL_VERIFIED" = false ]; then
  echo "❌ Some manifest signatures failed verification"
  exit 1
fi

# Sign each binary artifact
echo ""
echo "Signing binary artifacts ..."
cd out

# Generate checksums
sha256sum *.tar.gz > checksums.txt || true
echo "Checksums:"
cat checksums.txt

# Sign all tarballs and checksum file
SIGNING_FAILED=false
for file in *.tar.gz checksums.txt; do
  if [ -f "$file" ]; then
    echo "Signing $file..."
    if gpg --batch --yes --pinentry-mode loopback \
        --passphrase "$GPG_PASSPHRASE" \
        --armor --detach-sign \
        --local-user "$KEY_ID" \
        "$file" 2>/dev/null; then

      if [ -f "$file.asc" ]; then
        echo "✅ Created signature: $file.asc"
      else
        echo "❌ Failed to sign: $file"
        SIGNING_FAILED=true
      fi
    else
      echo "❌ GPG signing failed for: $file"
      SIGNING_FAILED=true
    fi
  fi
done

if [ "$SIGNING_FAILED" = true ]; then
  echo "❌ Some binary files could not be signed. Aborting."
  exit 1
fi

# Verify signatures on binaries
echo ""
echo "Verifying binary signatures..."
for file in *.tar.gz checksums.txt; do
  if [ -f "$file.asc" ]; then
    if gpg --verify "$file.asc" "$file" 2>&1 | grep -q "Good signature"; then
      echo "✅ Valid signature: $file"
    else
      echo "❌ Invalid signature: $file"
      exit 1
    fi
  fi
done

# Move signed binaries to release-artifacts directory
echo ""
echo "Moving signed binaries to release-artifacts..."
mv *.tar.gz *.tar.gz.asc checksums.txt checksums.txt.asc ../release-artifacts/ 2>/dev/null || true

# Return to parent directory
cd ..

# List all release artifacts
echo ""
echo "All signed release artifacts:"
ls -lh release-artifacts/

echo "signing_success=true" >> $GITHUB_OUTPUT
