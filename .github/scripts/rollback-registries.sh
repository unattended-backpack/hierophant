#!/bin/sh
# Rollback container images from registries on workflow failure.
#
# This script attempts to delete pushed container images from all registries
# to maintain consistency when a release workflow fails partway through.
#
# Environment variables required:
#   - DO_DIGEST: DigitalOcean registry digest to rollback (optional)
#   - GHCR_DIGEST: GitHub Container Registry digest to rollback (optional)
#   - DH_DIGEST: Docker Hub digest to rollback (optional)
#   - DO_TOKEN: DigitalOcean API token
#   - CI_GH_CLASSIC_PAT: GitHub classic PAT
#   - DH_TOKEN: Docker Hub token
#   - DH_USERNAME: Docker Hub username
#   - DO_REGISTRY_NAME: DigitalOcean registry name
#   - IMAGE_NAME: Image name
#   - GITHUB_SHA: Git commit SHA
#   - GITHUB_REPOSITORY: Repository name (owner/repo)
set -e
echo "Attempting to rollback $ROLLBACK_IMAGE_KEY ..."

# Rollback DO if needed.
DO_ROLLBACK_SUCCESS=true
if [ -n "$DO_DIGEST" ]; then
  echo "Must rollback DO ..."
  # Use DO API directly to delete manifest by digest
  RESPONSE=$(curl -X DELETE \
    -H "Authorization: Bearer $DO_TOKEN" \
    -H "Content-Type: application/json" \
    "https://api.digitalocean.com/v2/registry/$DO_REGISTRY_NAME/repositories/$IMAGE_NAME/digests/$DO_DIGEST" \
    -w "\n%{http_code}" -s)

  HTTP_CODE=$(echo "$RESPONSE" | tail -n 1)
  if [ "$HTTP_CODE" = "204" ] || [ "$HTTP_CODE" = "200" ]; then
    echo "✅ Deleted $DO_DIGEST from DO"
  else
    echo "❌ Failed to delete from DO (HTTP $HTTP_CODE)"
    DO_ROLLBACK_SUCCESS=false
  fi
fi

# Rollback GHCR if needed.
GHCR_ROLLBACK_SUCCESS=true
if [ -n "$GHCR_DIGEST" ]; then
  echo "Must rollback GHCR..."

  # Extract owner and construct package name with repository namespace
  REPO_OWNER=$(echo "$GITHUB_REPOSITORY" | cut -d'/' -f1)
  REPO_NAME=$(echo "$GITHUB_REPOSITORY" | cut -d'/' -f2)
  PACKAGE_NAME="${REPO_NAME}%2F${IMAGE_NAME}"  # URL-encoded path: repo/image
  echo "Looking for package: $REPO_OWNER/$PACKAGE_NAME"

  # Try org first, fall back to user.
  VERSIONS=$(curl -s \
    -H "Authorization: Bearer $CI_GH_CLASSIC_PAT" \
    -H "Accept: application/vnd.github.v3+json" \
    "https://api.github.com/orgs/$REPO_OWNER/packages/container/$PACKAGE_NAME/versions")

  # Determine if we're using org or user endpoints.
  ENDPOINT_TYPE="orgs"
  if echo "$VERSIONS" | grep -q "Not Found"; then
    ENDPOINT_TYPE="users"
    VERSIONS=$(curl -s \
      -H "Authorization: Bearer $CI_GH_CLASSIC_PAT" \
      -H "Accept: application/vnd.github.v3+json" \
      "https://api.github.com/users/$REPO_OWNER/packages/container/$PACKAGE_NAME/versions")
  fi

  # Count how many versions have tags.
  TAGGED_VERSION_COUNT=$(echo "$VERSIONS" | \
    jq '[.[] | select(.metadata.container.tags and (.metadata.container.tags | length > 0))] | length' \
    2>/dev/null || echo "0")

  # Find the version ID for our SHA tag.
  VERSION_ID=$(echo "$VERSIONS" | jq -r \
    --arg sha "$GITHUB_SHA" \
    '.[] | select(.metadata.container.tags[]? == $sha) | .id' \
    2>/dev/null | head -1)

  if [ -n "$VERSION_ID" ] && [ "$VERSION_ID" != "null" ] && \
     [ "$VERSION_ID" != "" ]; then
    echo "Found version ID: $VERSION_ID"

    if [ "$TAGGED_VERSION_COUNT" = "1" ]; then
      echo "⚠️ This is the only tagged version. Deleting entire package ..."

      RESPONSE=$(curl -X DELETE \
        -H "Authorization: Bearer $CI_GH_CLASSIC_PAT" \
        -H "Accept: application/vnd.github.v3+json" \
        "https://api.github.com/$ENDPOINT_TYPE/$REPO_OWNER/packages/container/$PACKAGE_NAME" \
        -w "\n%{http_code}" -s)

      HTTP_CODE=$(echo "$RESPONSE" | tail -n 1)
      if [ "$HTTP_CODE" = "204" ] || [ "$HTTP_CODE" = "200" ]; then
        echo "✅ Deleted entire package from GHCR"
      else
        echo "❌ Failed to delete package (HTTP $HTTP_CODE)"
        GHCR_ROLLBACK_SUCCESS=false
      fi
    else
      echo "Found $TAGGED_VERSION_COUNT tagged versions. Deleting just version $VERSION_ID..."

      # Delete the specific version.
      RESPONSE=$(curl -X DELETE \
        -H "Authorization: Bearer $CI_GH_CLASSIC_PAT" \
        -H "Accept: application/vnd.github.v3+json" \
        "https://api.github.com/$ENDPOINT_TYPE/$REPO_OWNER/packages/container/$PACKAGE_NAME/versions/$VERSION_ID" \
        -w "\n%{http_code}" -s)

      HTTP_CODE=$(echo "$RESPONSE" | tail -n 1)
      if [ "$HTTP_CODE" = "204" ] || [ "$HTTP_CODE" = "200" ]; then
        echo "✅ Deleted version $VERSION_ID from GHCR"
      else
        echo "❌ Failed to delete from GHCR (HTTP $HTTP_CODE)"
        GHCR_ROLLBACK_SUCCESS=false
      fi
    fi
  else
    echo "❌ Could not find version ID for tag $GITHUB_SHA"
    GHCR_ROLLBACK_SUCCESS=false
  fi
fi

# Rollback Docker Hub if needed.
DH_ROLLBACK_SUCCESS=true
if [ -n "$DH_DIGEST" ]; then
  echo "Must rollback Docker Hub ..."
  TOKEN=$(curl -s -X POST \
    -H "Content-Type: application/json" \
    -d "{\"username\":\"$DH_USERNAME\",\"password\":\"$DH_TOKEN\"}" \
    "https://hub.docker.com/v2/users/login/" | jq -r .token)

  # Delete the specific tag.
  if [ -n "$TOKEN" ]; then
    RESPONSE=$(curl -X DELETE \
      -H "Authorization: Bearer $TOKEN" \
      "https://hub.docker.com/v2/repositories/$DH_USERNAME/$IMAGE_NAME/tags/$GITHUB_SHA/" \
      -w "\n%{http_code}" -s)

    HTTP_CODE=$(echo "$RESPONSE" | tail -n 1)
    if [ "$HTTP_CODE" = "204" ] || [ "$HTTP_CODE" = "200" ]; then
      echo "✅ Deleted $GITHUB_SHA tag from Docker Hub"
    else
      echo "❌ Failed to delete from Docker Hub (HTTP $HTTP_CODE)"
      DH_ROLLBACK_SUCCESS=false
    fi
  else
    echo "❌ Failed to authenticate with Docker Hub"
    DH_ROLLBACK_SUCCESS=false
  fi
fi

# Output the final rollback status to GITHUB_ENV with image-specific keys
if [ -n "$GITHUB_ENV" ] && [ -n "$ROLLBACK_IMAGE_KEY" ]; then
  echo "${ROLLBACK_IMAGE_KEY}_DO_ROLLBACK_SUCCESS=${DO_ROLLBACK_SUCCESS}" >> $GITHUB_ENV
  echo "${ROLLBACK_IMAGE_KEY}_GHCR_ROLLBACK_SUCCESS=${GHCR_ROLLBACK_SUCCESS}" >> $GITHUB_ENV
  echo "${ROLLBACK_IMAGE_KEY}_DH_ROLLBACK_SUCCESS=${DH_ROLLBACK_SUCCESS}" >> $GITHUB_ENV
fi

# Create useful action logs.
echo "========================================="
if [ "$DO_ROLLBACK_SUCCESS" = true ]; then
  echo "✅ DO rollback success."
else
  echo "❌ DO rollback failure; manual intervention required."
fi
if [ "$GHCR_ROLLBACK_SUCCESS" = true ]; then
  echo "✅ GHCR rollback success."
else
  echo "❌ GHCR rollback failure; manual intervention required."
fi
if [ "$DH_ROLLBACK_SUCCESS" = true ]; then
  echo "✅ Docker Hub rollback success."
else
  echo "❌ DH rollback failure; manual intervention required."
fi
