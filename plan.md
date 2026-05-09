# nitpick-agent Requirements

This document captures the functionality `nitpick-agent` needs in order to cover the useful behavior from `reviewd`.

## 1. GitHub PR Discovery

`nitpick-agent` must discover PRs where the authenticated user has been requested for review.

Requirements:

- Poll GitHub for open PRs matching `user-review-requested:@me`.
- Support explicit review requests through the CLI/API.
- Avoid reviewing the same PR head repeatedly.
- Re-review a PR when its head SHA changes.

Current gap:

- `reviewd` polls GitHub for `user-review-requested:@me`.
- `nitpick-agent` can discover requested reviews, but the replacement workflow still needs to be verified end to end against the old behavior.
- `nitpick-agent` now records scheduled GitHub discovery failures in host status so the menu bar can surface missing `gh` or other review-source errors.

## 2. Processed PR Tracking

`nitpick-agent` must maintain a durable index of reviewed PR heads.

Requirements:

- Store owner, repo, PR number, and head SHA for each processed review.
- Mark a PR head as processed only after a successful review.
- Treat a changed head SHA as a new review requirement.
- Preserve processed state across daemon restarts.

Current gap:

- `reviewd` records reviewed PRs and head SHAs, then re-reviews when new commits arrive.
- `nitpick-agent` has local artifacts and processed review storage, but the operational behavior should remain requirement-driven and covered by integration tests.

## 3. GitHub PR Metadata Model

`nitpick-agent` must represent GitHub PRs as first-class review subjects.

Requirements:

- Track PR owner, repo, number, URL, title, author, state, and head SHA.
- Keep generic core review types independent from GitHub-specific fields where practical.
- Put GitHub-specific discovery, checkout, review-thread, and sync behavior in the GitHub adapter or a GitHub watcher layer.

Current gap:

- `reviewd` directly knows GitHub PR metadata.
- `nitpick-agent` now has GitHub PR metadata and state parsing in the GitHub adapter, while core remains generic.
- Checkout cleanup still needs to use the PR state model to remove durable checkouts for closed or merged PRs.

## 4. Local PR Checkout Management

`nitpick-agent` must provide a local checkout of the PR head when an agent reviews or responds.

Requirements:

- Clone the PR repository when no checkout exists.
- Fetch the PR head ref before review.
- Check out the current PR head into a stable per-PR directory.
- Retain checkouts durably under the agent data directory until cleanup can use closed/merged PR state.
- Pass the checkout directory to the provider as `repo_dir`.
- Reuse the checkout for follow-up responses when possible.

Current gap:

- `reviewd` clones, fetches, and checks out PR branches under `/tmp/reviewd`.
- `nitpick-agent` now uses durable checkout storage and has a GitHub cleanup API for closed or merged PR checkouts.
- `nitpick cleanup-checkouts` now runs an explicit host maintenance pass over known PR checkouts and records completed cleanup activities.
- The scheduled review-source poller now runs checkout cleanup after due polls for the normal GitHub-backed daemon.

References:

- `reviewd/lib/session.sh:30`
- `nitpick-agent/crates/nitpick-agent-github/src/lib.rs:176`

## 5. Real Provider Session Continuity

`nitpick-agent` must preserve and resume provider sessions across review and follow-up actions.

Requirements:

- Generate stable provider session IDs for PR review activities.
- Pass session IDs to Claude/Codex when starting a review, when the provider supports it.
- Resume existing provider sessions for re-review and comment-response work.
- Recover cleanly when a stored provider session no longer exists.
- Store the provider session ID in local activity/session state.

Current gap:

- `reviewd` uses explicit Claude `--session-id` and `--resume`.
- `nitpick-agent` stores a session object, but the command provider currently pipes a prompt to `claude`/`codex` stdin without session/resume flags.

References:

- `reviewd/lib/session.sh:53`
- `nitpick-agent/crates/nitpick-agent-core/src/command_provider.rs:36`

## 6. GitHub Draft Review Workflow

`nitpick-agent` must support GitHub review comments and pending review creation, not only standalone PR comments.

Requirements:

- Create or reuse a pending GitHub review for the authenticated user.
- Add inline draft review comments for file/line-specific findings.
- Keep standalone PR comments as a separate sync mode, not the default code-review workflow.
- Constrain GitHub write operations so the agent can add review comments without broad mutation rights.
- Preserve local artifacts as the source of truth before sync.

Current gap:

- `reviewd` creates pending GitHub reviews through the PR reviews API and uses review-safe `gh` wrappers.
- `nitpick-agent` currently syncs artifacts with `gh pr comment`, which posts standalone comments instead of draft inline review comments.

Reference:

- `nitpick-agent/crates/nitpick-agent-github/src/lib.rs:383`

## 7. Bot Review Duplicate Detection

`nitpick-agent` must avoid duplicate bot reviews for the same PR head.

Requirements:

- Detect existing bot-authored or bot-marked reviews for a PR.
- Skip normal review when the current PR head has already been reviewed.
- Treat changed heads as re-review candidates.
- Distinguish manual self-review from automatic duplicate prevention.

Current gap:

- `reviewd` skips normal reviews when a bot review already exists and treats later runs as re-reviews.
- `nitpick-agent` does not appear to have equivalent bot-review detection.

## 8. Comment Watcher And Response Loop

`nitpick-agent` must respond to relevant PR review-thread comments.

Requirements:

- Fetch unresolved review threads for a PR.
- Identify threads that need an agent response:
  - a human replied after a bot response
  - a comment mentions `@claude`
- Resume the existing PR review session when responding.
- Fall back to a new session when the prior provider session is unavailable.
- Add responses as draft review comments on the relevant file/line.
- Stop watching closed or merged PRs.

Current gap:

- `reviewd` monitors unresolved threads, detects `@claude` comments or replies to bot comments, and responds.
- `nitpick-agent` has no equivalent GraphQL thread fetch or comment-response workflow.

Reference:

- `reviewd/lib/watcher.sh:89`

## 9. Operational Logs And Resume UX

`nitpick-agent` must expose enough operational state to debug and resume reviews.

Requirements:

- Show daemon status.
- Show active/running/completed/error review activities.
- Provide per-PR logs or activity logs.
- Provide daemon logs.
- Let the user resume or attach to an active provider session when supported.
- Avoid requiring tmux specifically, but preserve the useful resume/log workflows.

Current gap:

- `reviewd` supports `status`, `logs [pr]`, and `resume [pr]` with tmux-backed reviewer/watcher processes.
- `nitpick-agent` has status, activities, and artifacts, but not interactive attach/resume or per-PR process logs.
