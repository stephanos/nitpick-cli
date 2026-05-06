1. PR discovery
   reviewd polls GitHub for PRs where user-review-requested:@me.
   We currently only review when the user explicitly runs CLI/API review.
2. Processed PR tracking
   reviewd records reviewed PRs plus head SHA, then re-reviews when new commits arrive.
   We have local artifacts, but no “this PR/head SHA has been reviewed” index.
3. GitHub PR metadata model
   reviewd knows PR owner/repo/number, URL, title, author, state, head SHA.
   Our core intentionally stays generic, and the GitHub crate only has parsing + posting. We need a GitHub-specific watcher layer.
4. Logs and resume UX
   reviewd logs [pr] is useful operational tools.
   Our CLI has activities/artifacts, but no logs.
5. Real GitHub review creation
   reviewd creates pending GitHub reviews via the PR reviews API.
   We currently post a PR comment through gh pr comment; that’s simpler but not the same as inline/pending review workflow.
