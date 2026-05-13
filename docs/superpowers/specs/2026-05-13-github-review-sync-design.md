# GitHub review-sync draft workflow design

## Problem

`nitpick review-sync <activity-id> <pr-ref>` currently submits one GitHub `COMMENT` review immediately. The remaining design gap is how review sync should work when the target is a GitHub pending draft review instead of an immediately submitted review.

The chosen direction is a conservative draft workflow:

- `review-sync` stages one pending GitHub review draft.
- Submission remains a manual user action in the GitHub UI.
- nitpick must never delete or replace an existing draft review automatically.
- nitpick must not guess about unsupported GitHub review mutations.

## Goals

- Support staging review summaries and inline review comments into a pending GitHub review.
- Preserve local artifacts as the source of truth.
- Avoid destructive behavior against existing draft reviews.
- Reconcile local state when the user manually submits the draft in GitHub.

## Non-goals

- Automatic review submission from nitpick.
- Automatic draft deletion or recreation.
- Support for multiple pending draft reviews per activity.
- Adding new inline comments to an existing pending draft unless GitHub explicitly supports it through a stable API path.

## Current constraints

- GitHub supports creating a pending pull request review by omitting `event` from `POST /repos/{owner}/{repo}/pulls/{pull_number}/reviews`.
- GitHub supports updating a review summary body with `PUT /repos/{owner}/{repo}/pulls/{pull_number}/reviews/{review_id}`.
- GitHub supports submitting a pending review, but nitpick will not do that in this workflow.
- GitHub documentation does not clearly expose a supported way to append new inline review comments to an already existing pending review by review ID.

Because of that last constraint, the design must prefer refusal over destructive replacement.

## User-facing behavior

### First sync

When an activity has no staged GitHub draft review for the target PR:

1. `review-sync` creates one pending GitHub review containing the current review summary and all current inline review comments.
2. nitpick stores the remote draft review handle locally.
3. The staged artifacts move to a pending `github-review` sync state tied to that remote draft.

### Re-sync while the draft is still pending

When nitpick finds an existing pending draft review for the same activity and target PR:

- If only the review summary changed, nitpick updates the remote draft review body.
- If new inline comment artifacts exist that are not already staged in the remote draft, nitpick refuses to modify the remote draft and returns a clear message telling the user to submit or manually clear the draft review before staging more review comments.
- nitpick does not delete the draft and does not recreate it.

### Manual submission in GitHub

The user submits the pending review in GitHub.

On the next `review-sync` for that activity and PR:

1. nitpick loads the stored review handle.
2. If GitHub reports that the review is no longer `PENDING`, nitpick marks the associated staged artifacts as `Synced` using the remote review URL or ID.
3. Any newer local-only artifacts are then eligible for a fresh pending draft review.

### Missing draft

If nitpick has a stored pending draft review handle but GitHub no longer returns that review:

- nitpick clears the stored pending draft association.
- The previously staged artifacts return to local-only state.
- nitpick does not assume whether the user deleted the draft or whether the draft was lost for another reason.

### Head SHA changes

If the PR head SHA changed after a draft was staged:

- nitpick refuses to mutate the existing draft review.
- nitpick tells the user to submit or clear the old draft manually before staging new review comments for the new head.

This avoids silently mixing comments prepared for different commits.

## Architecture and state model

### Artifact sync state

The existing `ArtifactSyncState::Pending` state should be extended so pending `github-review` artifacts can retain the remote draft review handle they belong to. That handle is needed to:

- inspect the current GitHub draft review state on later syncs,
- reconcile manual submission,
- distinguish a known staged draft from generic unsent work.

The handle may be stored as the GitHub review ID plus the HTML URL, or as a structured destination-specific payload if that proves cleaner.

### Remote draft membership

Each artifact staged into the draft review should retain the same remote draft handle. This lets nitpick determine which artifacts belong to the same pending draft review without introducing destructive batch replacement behavior.

### Source of truth

Local artifacts remain authoritative. Remote draft state is treated as a projection of the local staged subset, not as the canonical record of the review.

## Sync algorithm

For `review-sync <activity-id> <pr-ref>` against `github-review`:

1. Load activity artifacts.
2. Partition artifacts into:
   - local-only review artifacts,
   - pending `github-review` artifacts associated with a stored draft handle,
   - already-synced artifacts.
3. If no pending draft handle exists:
   - create a pending GitHub review from the current local-only review artifacts,
   - store the returned draft handle on those artifacts,
   - mark them pending.
4. If a pending draft handle exists:
   - fetch the remote review by handle,
   - if the review is submitted, mark its associated local pending artifacts as synced, then continue with any local-only artifacts as a new draft candidate,
   - if the review is missing, clear the handle from its associated local pending artifacts and return them to local-only,
   - if the review is still pending on the same head:
     - update the summary body if needed,
     - refuse the sync if unstaged inline comment artifacts exist,
     - otherwise leave the existing draft intact.
5. Return status that makes it clear whether work was staged, reconciled, refused, or left unchanged.

## Error handling

- **Unsupported mutation path:** If nitpick cannot safely add new inline comments to an existing pending draft, it returns a specific refusal instead of mutating or replacing the draft.
- **Remote review missing:** Clear the draft handle and restore affected artifacts to local-only.
- **Remote review submitted:** Promote affected artifacts to synced.
- **Head mismatch:** Refuse to mutate the draft and explain why.
- **Mixed handles in one activity:** Return an explicit error because that indicates inconsistent local state.
- **GitHub API or CLI failure:** Surface the exact failure and preserve the existing local artifact state.

## CLI and API implications

- `nitpick review-sync <activity-id> <pr-ref>` changes meaning from "submit one review now" to "stage or reconcile a pending GitHub draft review safely."
- `artifact-sync <artifact-id> github-review <pr-ref>` may continue to support first-time staging for a single review artifact, but it should follow the same non-destructive pending-draft rules.
- CLI output should tell the user whether the command:
  - created a draft,
  - updated the draft summary,
  - detected manual submission and marked artifacts synced,
  - refused to stage new inline comments because a pending draft already exists.

## Testing

Add coverage for:

1. creating the first pending draft review from one summary plus inline comments,
2. re-syncing with only a changed summary body and updating the draft body,
3. re-syncing with new inline comments while a pending draft exists and getting a refusal,
4. reconciling a manually submitted draft into synced artifact state,
5. clearing a missing remote draft back to local-only state,
6. refusing mutation when the PR head SHA no longer matches the draft review commit,
7. surfacing an error when pending artifacts within one activity point at different remote draft handles.

## Notes

- This design deliberately prefers conservative behavior over clever recovery.
- If GitHub later exposes a documented way to append inline comments to an existing pending review, nitpick can loosen the refusal path without changing the higher-level workflow.
