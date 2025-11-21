/**
  Create a GitHub issue documenting a failed release and rollback status.

  This script creates an issue when the release workflow fails, documenting:
  - Build success/failure status
  - Which registries were successfully pushed to
  - Which rollbacks succeeded or require manual intervention

  @param {Object} params - GitHub Actions script context
  @param {Object} params.github - Pre-authenticated GitHub API client
  @param {Object} params.context - Workflow context
  @param {Object} params.core - GitHub Actions core utilities

  @returns {Promise<void>}
*/
module.exports = async ({ github, context, core }) => {

  // Get workflow data from environment
  const shaShort = process.env.BUILD_SHA_SHORT;
  const timestamp = process.env.BUILD_TIMESTAMP;
  const buildSuccess = process.env.BUILD_SUCCESS === 'true';
  const releaseSuccess = process.env.RELEASE_SUCCESS === 'true';

  // Dynamically discover all images from environment variables.
  const images = {};
  const envKeys = Object.keys(process.env);
  for (const key of envKeys) {
    const match = key.match(/^(.+)_DO_DIGEST$/);
    if (match) {
      const imageName = match[1];
      images[imageName] = {
        doDigest: process.env[`${imageName}_DO_DIGEST`],
        ghcrDigest: process.env[`${imageName}_GHCR_DIGEST`],
        dhDigest: process.env[`${imageName}_DH_DIGEST`],
        doRollback: process.env[`${imageName}_DO_ROLLBACK_SUCCESS`] === 'true',
        ghcrRollback: process.env[`${imageName}_GHCR_ROLLBACK_SUCCESS`] === 'true',
        dhRollback: process.env[`${imageName}_DH_ROLLBACK_SUCCESS`] === 'true'
      };
    }
  }
  const imageNames = Object.keys(images);

  // Generate rollback summary for all discovered images.
  let rollbackSummary = '';
  for (const imageName of imageNames.sort()) {
    const image = images[imageName];
    const imageTitle = imageName.charAt(0).toUpperCase() + imageName.slice(1);
    const doRollbackText =
      `- DOCR Rollback: ${image.doRollback ? '✅' : '❌ manual intervention required.'}`;
    const ghcrRollbackText =
      `- GHCR Rollback: ${image.ghcrRollback ? '✅' : '❌ manual intervention required.'}`;
    const dhRollbackText =
      `- DHCR Rollback: ${image.dhRollback ? '✅' : '❌ manual intervention required.'}`;

    // Add this image's details to the rollback summary.
    rollbackSummary += `## ${imageTitle} Status

### Registry Pushes
- DOCR: ${image.doDigest ? `✅ \`${image.doDigest}\`` : '❌' }
- GHCR: ${image.ghcrDigest ? `✅ \`${image.ghcrDigest}\`` : '❌' }
- DHCR: ${image.dhDigest ? `✅ \`${image.dhDigest}\`` : '❌' }

### Registry Rollbacks
${image.doDigest ? doRollbackText : '' }
${image.ghcrDigest ? ghcrRollbackText : '' }
${image.dhDigest ? dhRollbackText : '' }
    `;
  }

  // Create the rollback issue.
  const workflowUrl = `${process.env.GITHUB_SERVER_URL}/${process.env.GITHUB_REPOSITORY}/actions`;
  const actor = process.env.GITHUB_ACTOR;
  await github.rest.issues.create({
    owner: context.repo.owner,
    repo: context.repo.repo,
    title: `⚠️ Release failed for ${shaShort}`,
    body: `# Status

Attention @${actor}, an automated release failed. This issue is generated to track the status of build success, partial releases, registry pushes and rollbacks. For full details please refer to [workflow logs](${workflowUrl}).

The automated release process attempts to build the project, push it to various container registries, ensure consistency between the container registries, and release the project.
1. If the build fails, nothing else happens.
2. If successful and consistent pushes to all container registries cannot be verified, a warning-laden partial release of the project is produced. The automated release process will attempt to restore container registry consistency by rolling back the mismatched state.
3. In the event that a registry push succeeded but its corresponding rollback failed, you will need to manually intervene to ensure consistent images between container registries.

## Build Status
- ${buildSuccess ? '✅ The build succeeded.' : '❌ The build failed.'}
- ${releaseSuccess ? '⚠️ A release was made.' : '✅ No release was made.'}

${rollbackSummary}
`,
    labels: ['release-failure', 'needs-investigation']
  });

  console.log('Created rollback tracking issue');
};
