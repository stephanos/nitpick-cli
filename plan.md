# nitpick-agent Requirements

This document tracks the functionality `nitpick-agent` needs in order to cover the useful behavior from `reviewd`.

## 1. GitHub PR Discovery

`nitpick-agent` must discover PRs where the authenticated user has been requested for review.

Status: Implemented

Implemented:

- Polls GitHub for open PRs matching `user-review-requested:@me`.
- Supports explicit review requests through the CLI/API.
- Avoids reviewing the same PR head repeatedly.
- Re-reviews a PR when its head SHA changes.
- Records scheduled GitHub discovery failures in host status so the menu bar can surface missing `gh` or other review-source errors.

Remaining:

- Verify the replacement workflow end to end against the old `reviewd` behavior in a real GitHub account/repository setup.

## 2. Processed PR Tracking

`nitpick-agent` must maintain a durable index of reviewed PR heads.

Status: Implemented

Implemented:

- Stores owner, repo, PR number, and head SHA for each processed review.
- Marks a PR head as processed only after a successful review.
- Treats a changed head SHA as a new review requirement.
- Preserves processed state across daemon restarts.

Remaining:

- None currently known.

## 3. GitHub PR Metadata Model

`nitpick-agent` must represent GitHub PRs as first-class review subjects.

Status: Implemented

Implemented:

- Tracks PR owner, repo, number, URL, title, author, state, and head SHA.
- Keeps generic core review types independent from GitHub-specific fields.
- Keeps GitHub-specific discovery, checkout, cleanup, and sync behavior in the GitHub adapter/host layer.

Remaining:

- None currently known.

## 4. Local PR Checkout Management

`nitpick-agent` must provide a local checkout of the PR head when an agent reviews or inspects a PR.

Status: Implemented

Implemented:

- Clones the PR repository when no checkout exists.
- Fetches the PR head ref before review.
- Checks out the current PR head into a stable per-PR directory.
- Retains checkouts durably under the agent data directory.
- Passes the checkout directory to the provider as `repo_dir`.
- Cleans up closed or merged PR checkouts through `nitpick cleanup-checkouts`.
- Runs scheduled checkout cleanup after due GitHub discovery polls.
- Records completed cleanup activities.
- Opens an existing durable PR checkout with `nitpick inspect <pr-ref>`.

Remaining:

- Follow-up response workflows may need to reuse the checkout if comment-response support is reintroduced.

References:

- `reviewd/lib/session.sh:30`
- `nitpick-agent/crates/nitpick-agent-github/src/lib.rs`

## 5. Real Provider Session Continuity

`nitpick-agent` must preserve and resume provider sessions across review and follow-up actions.

Status: Implemented for current review flow

Implemented:

- Generates stable provider session IDs for PR review activities.
- Passes stable session IDs to Claude reviews with `--session-id`.
- Stores provider session IDs in local activity/session state.
- Reopens stored Claude sessions through `nitpick resume <activity-id|pr-ref>`.
- Reopens stored Codex sessions through `nitpick resume <activity-id|pr-ref>`.

Remaining:

- Recover gracefully when a stored provider session no longer exists.
- Reuse provider sessions for comment-response work if that workflow is reintroduced.

References:

- `reviewd/lib/session.sh:53`
- `nitpick-agent/crates/nitpick-agent-core/src/command_provider.rs`

## 6. GitHub Draft Review Workflow

`nitpick-agent` must support GitHub review comments and pending review creation, not only standalone PR comments.

Status: Partial

Implemented:

- Keeps standalone PR comments as a separate sync mode through `github`.
- Syncs a single review artifact with `artifact-sync <artifact-id> github-review <pr-ref>`.
- Syncs all review artifacts from one activity as a single GitHub pull request review with `nitpick review-sync <activity-id> <pr-ref>`.
- Preserves local artifacts as the source of truth before sync.
- Uses `gh pr review` and the GitHub pull request review API for review-specific writes.

Remaining:

- Document the minimum GitHub token scopes/permissions needed for review sync.

References:

- `reviewd` pending-review behavior.
- `nitpick-agent/crates/nitpick-agent-github/src/lib.rs`

## 7. Bot Review Duplicate Detection

`nitpick-agent` must avoid duplicate bot reviews for the same PR head.

Status: Implemented

Implemented:

- Detects existing nitpick-marked reviews for the current PR head.
- Skips automatic review when GitHub already has a nitpick-marked review on the current PR head.
- Treats changed heads as re-review candidates.
- Leaves manual reviews unaffected by automatic duplicate prevention.

Remaining:

- None currently known.

## 8. Operational Logs And Resume UX

`nitpick-agent` must expose enough operational state to debug and resume reviews.

Status: Implemented

Implemented:

- Shows daemon status.
- Lists active/running/completed/error review activities with updated time, activity ID, provider session ID, and errors.
- Shows per-activity and per-PR logs with `nitpick logs <activity-id|pr-ref>`.
- Shows daemon logs with `nitpick logs daemon`.
- Opens durable checkouts with `nitpick inspect <pr-ref>`.
- Reopens supported provider sessions with `nitpick resume <activity-id|pr-ref>`.
- Avoids requiring tmux while preserving the useful status/log/resume workflows.

Remaining:

- None currently known.
