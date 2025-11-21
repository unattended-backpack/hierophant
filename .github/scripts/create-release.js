/**
  Create a GitHub release with multiple container images and GPG-signed artifacts.
  Images are discovered dynamically from environment variables.

  @param {Object} params - GitHub Actions script context
  @param {Object} params.github - Pre-authenticated GitHub API client
  @param {Object} params.context - Workflow context
  @param {Object} params.core - GitHub Actions core utilities

  @returns {Promise<void>}
*/
module.exports = async ({ github, context, core }) => {
  const fs = require('fs');
  const path = require('path');

  // Get workflow inputs from environment
  const sha = process.env.GITHUB_SHA;
  const timestamp = process.env.BUILD_TIMESTAMP;
  const shaShort = process.env.BUILD_SHA_SHORT;
  const repository = process.env.GITHUB_REPOSITORY;
  const repositoryOwner = process.env.GITHUB_REPOSITORY_OWNER;
  const doRegistryName = process.env.DO_REGISTRY_NAME;
  const dhUsername = process.env.DH_USERNAME;
  const releaseNotes = process.env.RELEASE_NOTES || '';
  const gpgPublicKey = process.env.GPG_PUBLIC_KEY || '';
  const isAct = process.env.ACT === 'true';

  // Dynamically discover all images from environment variables.
  const images = {};
  const envKeys = Object.keys(process.env);
  for (const key of envKeys) {
    const match = key.match(/^(.+)_DO_DIGEST$/);
    if (match) {
      const matrixName = match[1];
      const publishedName = process.env[`${matrixName}_PUBLISHED_NAME`] || matrixName;
      images[matrixName] = {
        publishedName: publishedName,
        doDigest: process.env[`${matrixName}_DO_DIGEST`],
        ghcrDigest: process.env[`${matrixName}_GHCR_DIGEST`],
        dhDigest: process.env[`${matrixName}_DH_DIGEST`],
        imageId: process.env[`${matrixName}_IMAGE_ID`]
      };
    }
  }
  const imageNames = Object.keys(images);
  if (imageNames.length === 0) {
    console.log('No images found in environment variables, skipping release.');
    return;
  }
  console.log(`Found ${imageNames.length} images: ${imageNames.join(', ')}`);

  // Check if all required digests are present
  for (const imageName of imageNames) {
    const img = images[imageName];
    if (!img.doDigest || !img.ghcrDigest || !img.dhDigest) {
      console.log(`Missing registry pushes for ${imageName}, skipping release.`);
      return;
    }
  }

  // Build container images section dynamically.
  let containerImagesSection = '## Container Images\n\n';
  containerImagesSection += 'Images have been pushed to the following container registries; some may be private.\n\n';

  for (const matrixName of imageNames.sort()) {
    const img = images[matrixName];
    const publishedName = img.publishedName;
    const imageTitle = matrixName.charAt(0).toUpperCase() + matrixName.slice(1);
    containerImagesSection += `### ${imageTitle}${publishedName !== matrixName ? ` (published as ${publishedName})` : ''}\n\n`;
    containerImagesSection += `${img.ghcrDigest ? `- GHCR: \`ghcr.io/${repository}/${publishedName}:${sha}\`` : '- GHCR: ❌'}\n`;
    containerImagesSection += `${img.dhDigest ? `- DHCR: \`${dhUsername}/${publishedName}:${sha}\`` : '- DHCR: ❌'}\n`;
    containerImagesSection += `${img.doDigest ? `- DOCR: \`registry.digitalocean.com/${doRegistryName}/${publishedName}:${sha}\`` : '- DOCR: ❌'}\n\n`;

    containerImagesSection += '```bash\n';
    containerImagesSection += `docker pull ghcr.io/${repository}/${publishedName}@${img.ghcrDigest}\n`;
    containerImagesSection += `docker pull ${dhUsername}/${publishedName}@${img.dhDigest}\n`;
    containerImagesSection += `docker pull registry.digitalocean.com/${doRegistryName}/${publishedName}@${img.doDigest}\n`;
    containerImagesSection += '```\n\n';
    containerImagesSection += `After pulling from a registry, verify the image ID matches \`${img.imageId}\` by running \`docker inspect ${publishedName} --format='{{.Id}}'\`.\n\n`;
  }

  // Build GPG verification section dynamically (use matrix names for file references).
  let gpgManifestList = '';
  for (const matrixName of imageNames.sort()) {
    const publishedName = images[matrixName].publishedName;
    gpgManifestList += `- \`${matrixName}-ghcr-manifest.json\` - The complete GHCR ${publishedName} image manifest.\n`;
    gpgManifestList += `- \`${matrixName}-dh-manifest.json\` - The complete Docker Hub ${publishedName} image manifest.\n`;
    gpgManifestList += `- \`${matrixName}-do-manifest.json\` - The complete DigitalOcean ${publishedName} image manifest.\n`;
  }

  let gpgVerifyCommands = '';
  for (const matrixName of imageNames.sort()) {
    gpgVerifyCommands += `gpg --verify ${matrixName}-ghcr-manifest.json.asc ${matrixName}-ghcr-manifest.json\n`;
    gpgVerifyCommands += `gpg --verify ${matrixName}-dh-manifest.json.asc ${matrixName}-dh-manifest.json\n`;
    gpgVerifyCommands += `gpg --verify ${matrixName}-do-manifest.json.asc ${matrixName}-do-manifest.json\n`;
  }

  // Build cosign verification section dynamically (use published names for verification).
  let cosignVerifyCommands = '';
  for (const matrixName of imageNames.sort()) {
    const img = images[matrixName];
    const publishedName = img.publishedName;

    cosignVerifyCommands += `# Verify GHCR ${publishedName} image\n`;
    cosignVerifyCommands += `cosign verify ghcr.io/${repository}/${publishedName}@${img.ghcrDigest} \\\\\n`;
    cosignVerifyCommands += `  --certificate-identity-regexp='^https://github.com/${repository.split('/')[0]}/.+' \\\\\n`;
    cosignVerifyCommands += `  --certificate-oidc-issuer=https://token.actions.githubusercontent.com\n\n`;

    cosignVerifyCommands += `# Verify Docker Hub ${publishedName} image\n`;
    cosignVerifyCommands += `cosign verify ${dhUsername}/${publishedName}@${img.dhDigest} \\\\\n`;
    cosignVerifyCommands += `  --certificate-identity-regexp='^https://github.com/${repository.split('/')[0]}/.+' \\\\\n`;
    cosignVerifyCommands += `  --certificate-oidc-issuer=https://token.actions.githubusercontent.com\n\n`;

    cosignVerifyCommands += `# Verify DigitalOcean ${publishedName} image\n`;
    cosignVerifyCommands += `cosign verify registry.digitalocean.com/${doRegistryName}/${publishedName}@${img.doDigest} \\\\\n`;
    cosignVerifyCommands += `  --certificate-identity-regexp='^https://github.com/${repository.split('/')[0]}/.+' \\\\\n`;
    cosignVerifyCommands += `  --certificate-oidc-issuer=https://token.actions.githubusercontent.com\n\n`;
  }

  // Prepare the release body
  const body = `## Release Notes

${releaseNotes}

${containerImagesSection}
## Native Binaries

Pre-built native binaries are available as release assets:
- Binary tarballs: \`*_${shaShort}_*.tar.gz\`.
- SHA256 checksums: \`checksums.txt\`.

All binaries include \`.asc\` signature files for GPG verification.

## GPG Signature Verification

All release artifacts are signed with GPG, including:
- \`image-digests.txt\` - A human-readable digest list for all images.
${gpgManifestList}- \`*.tar.gz\` - Native binary tarballs.
- \`checksums.txt\` - SHA256 checksums for binaries.

Download the artifacts and their \`.asc\` signature files from the release assets below. To verify authenticity, copy this public key \`${gpgPublicKey}\` into a \`public.asc\` file and verify the signatures:

\`\`\`bash
# Import GPG public key.
cat public.asc | base64 -d | gpg --import

# Verify digest list.
gpg --verify image-digests.txt.asc image-digests.txt

# Verify image manifests.
${gpgVerifyCommands}
# Verify binary checksums.
gpg --verify checksums.txt.asc checksums.txt

# Verify binaries.
gpg --verify *.tar.gz.asc
\`\`\`

Valid signatures confirm the artifacts were signed by the maintainer. The manifest signatures provide cryptographic proof of the complete image structure, while binary signatures ensure the authenticity of native executables.
${!isAct ? `
## Cosign Verification (Optional)

Images are also signed with [cosign](https://github.com/sigstore/cosign) using GitHub Actions OIDC for automated verification and build provenance:

\`\`\`bash
${cosignVerifyCommands}\`\`\`

Cosign provides automated verification without manual key management. Signatures prove the images were built by this repository's GitHub Actions workflow and are stored in the [Rekor transparency log](https://search.sigstore.dev/).

**Note**: Cosign depends on external infrastructure (GitHub OIDC, Rekor). For maximum trust independence, rely on the GPG-signed manifests as your ultimate root of trust.
` : ''}
`;

  // Create the release as a draft first
  const release = await github.rest.repos.createRelease({
    owner: context.repo.owner,
    repo: context.repo.repo,
    tag_name: `${timestamp}-${shaShort}`,
    name: `Release ${shaShort}`,
    body: body,
    draft: true,
    prerelease: false,
    target_commitish: sha
  });
  console.log(`Created draft release: ${release.data.html_url}`);

  // Upload signed artifacts
  const artifactsDir = 'release-artifacts';
  const artifacts = fs.readdirSync(artifactsDir);
  for (const artifact of artifacts) {
    const artifactPath = path.join(artifactsDir, artifact);
    const stats = fs.statSync(artifactPath);

    if (stats.isFile()) {
      console.log(`Uploading artifact: ${artifact}`);
      await github.rest.repos.uploadReleaseAsset({
        owner: context.repo.owner,
        repo: context.repo.repo,
        release_id: release.data.id,
        name: artifact,
        data: fs.readFileSync(artifactPath)
      });
    }
  }

  // Publish the release
  console.log('Publishing release...');
  await github.rest.repos.updateRelease({
    owner: context.repo.owner,
    repo: context.repo.repo,
    release_id: release.data.id,
    draft: false
  });
  console.log(`✅ Published release: ${release.data.html_url}`);

  core.setOutput('RELEASE_SUCCESS', true);
  core.setOutput('RELEASE_ID', release.data.id);
};
