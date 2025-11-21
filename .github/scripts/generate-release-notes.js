/**
  This helper function extracts sections of text from a PR body.
*/
function extractSection (text, sectionName) {
  const regex = new RegExp(
    `##\\s*${sectionName}\\s*([\\s\\S]*?)(?=##|$)`, 'i');
  const match = text.match(regex);
  return match ? match[1].trim() : '';
}

/**
  Generate release notes from PR body and commit history.

  @param {Object} params - GitHub Actions script context
  @param {Object} params.github - Pre-authenticated GitHub API client
  @param {Object} params.context - Workflow context
  @param {Object} params.core - GitHub Actions core utilities

  @returns {Promise<string>} Release notes markdown
*/
module.exports = async ({ github, context, core }) => {
  console.log('Extracting release notes ...');

  // Find the last merged PR.
  const prs = await github.rest.pulls.list({
    owner: context.repo.owner,
    repo: context.repo.repo,
    state: 'closed',
    sort: 'updated',
    direction: 'desc',
    per_page: 10
  });
  const mergedPR = prs.data.find(pr =>
    pr.merged_at &&
    pr.merge_commit_sha === context.sha
  );

  // If a PR was merged, include its information in the release notes.
  let releaseNotes = '';
  let prNumber = '';
  let prAuthor = '';
  if (mergedPR) {
    console.log(`Found merged PR #${mergedPR.number}: ${mergedPR.title}`);

    // Extract PR details.
    prNumber = mergedPR.number;
    prAuthor = mergedPR.user.login;
    const prTitle = mergedPR.title;
    const body = mergedPR.body || '';
    const description = extractSection(body, 'Description') || prTitle;
    const breakingChanges = extractSection(body, 'Breaking Changes');
    const bugFixes = extractSection(body, 'Bug Fixes');
    const features = extractSection(body, 'Features');

    // Construct the full release notes.
    releaseNotes = `### Description\n\n${description}`;
    if (breakingChanges) {
      releaseNotes += `\n\n### Breaking Changes\n\n${breakingChanges}`;
    }
    if (bugFixes) {
      releaseNotes += `\n\n### Bug Fixes\n\n${bugFixes}`;
    }
    if (features) {
      releaseNotes += `\n\n### Features\n\n${features}`;
    }
    if (
      releaseNotes === `### Description\n\n${prTitle}` &&
      !breakingChanges && !bugFixes && !features
    ) {
      releaseNotes = '';
    }
  }

  // Construct the full conventional commits changelog.
  let fullChanges = '';
  try {

    // Find the previous tag to compare against.
    const tags = await github.rest.repos.listTags({
      owner: context.repo.owner,
      repo: context.repo.repo,
      per_page: 1
    });
    const previousTag = tags.data.length > 0 ? tags.data[0].name : '';
    console.log(`Previous tag: ${previousTag || 'none'}`);
    console.log(`Current SHA: ${context.sha}`); 
    const compareUrl =
      `https://github.com/${context.repo.owner}/${context.repo.repo}/` +
      `compare/${previousTag}...${context.sha}`;

    // Find all commits since the previous tag.
    const commits = await github.rest.repos.compareCommits({
      owner: context.repo.owner,
      repo: context.repo.repo,

      // Compare to a hardcoded empty tree if there is no previous tag.
      base: previousTag || '4b825dc642cb6eb9a060e54bf8d69288fbee4904',
      head: context.sha
    });

    // Categorize the commits.
    const featCommits = [];
    const fixCommits = [];
    const otherCommits = [];
    commits.data.commits.forEach(commit => {
      const msg = commit.commit.message;
      const firstLine = msg.split('\n')[0];

      // Format features together.
      if (firstLine.startsWith('feat:') || firstLine.startsWith('feat(')) {
        featCommits.push(
          `- ${firstLine.replace(/^feat(\(.*?\))?:\s*/, '')} ` +
          `(${commit.author?.login || commit.commit.author.name})`);
      
      // Format fixes together.
      } else if (firstLine.startsWith('fix:') || firstLine.startsWith('fix(')) {
        fixCommits.push(
          `- ${firstLine.replace(/^fix(\(.*?\))?:\s*/, '')} ` +
          `(${commit.author?.login || commit.commit.author.name})`);
      
      // Dump everything else into its own section.
      } else if (!firstLine.startsWith('Merge')) {
        otherCommits.push(
          `- ${firstLine} ` +
          `(${commit.author?.login || commit.commit.author.name})`);
      }
    });

    // Add any commits to the release notes.
    if (featCommits.length > 0) {
      fullChanges += '**Features:**\n' + featCommits.join('\n') + '\n\n';
    }
    if (fixCommits.length > 0) {
      fullChanges += '**Bug Fixes:**\n' + fixCommits.join('\n') + '\n\n';
    }
    if (otherCommits.length > 0) {
      fullChanges += '**Other:**\n' + otherCommits.join('\n') + '\n\n';
    }
    fullChanges += `**Full Changelog:** ${compareUrl}`;

  // Provide a fallback changelog of full commits.
  } catch (error) {
    console.log('Cannot generate changelog:', error.message);
    fullChanges =
      '**Full Changelog**: see commit history for details.';
  }

  // Return the combined release notes.
  let finalNotes = releaseNotes;
  if (fullChanges) {
    finalNotes = finalNotes ? `${finalNotes}\n\n${fullChanges}` : fullChanges;
  }
  if (prNumber) {
    finalNotes += `\n\n---\n**PR:** #${prNumber} by ` + `@${prAuthor}`;
  }
  return finalNotes;
};
