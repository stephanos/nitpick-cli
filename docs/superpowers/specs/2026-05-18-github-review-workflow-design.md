# GitHub Review Workflow Design

## Summary

Deepen the GitHub integration in `nitpick-agent-github` into one **GitHub review workflow** module that owns the full pull-request flow: discovery, preparation of `ReviewInput`, sync of review artifacts, and checkout cleanup.

This keeps the existing `ReviewSource` and `ArtifactSyncDestination` seams from `nitpick-agent-core`, but moves GitHub workflow knowledge behind a deeper interface with better **locality** and **leverage**.

## Goals

- Concentrate GitHub pull-request workflow behavior in one module
- Hide checkout management inside the implementation
- Keep the existing outer seams from `nitpick-agent-core`
- Use a real seam with two adapters:
  - production adapter using `gh` and `git`
  - test adapter with deterministic workflow behavior
- Make the module interface the main test surface

## Non-Goals

- Redesign `ReviewSource` or `ArtifactSyncDestination`
- Change the storage model or host API contract
- Generalize beyond GitHub in this refactor

## Current Friction

`crates/nitpick-agent-github/src/lib.rs` currently mixes:

- review-request discovery
- pull-request metadata loading
- diff loading
- checkout creation and refresh
- artifact sync to GitHub comments and reviews
- pending-review updates
- cleanup of old checkouts
- command execution and error parsing

The result is a shallow interface spread across several public types (`GitHubCliDiscovery`, `GitHubCliSyncDestination`, `GitHubCliReviewSyncDestination`) plus many helpers. Callers and maintainers must bounce between discovery, sync, parsing, and checkout logic to understand one concept.

## Proposed Design

### Module Shape

Introduce a deep **GitHub review workflow** module inside `nitpick-agent-github` as the main owner of GitHub pull-request behavior.

Its interface should cover:

- discovering review requests
- checking whether a review request was already reviewed
- preparing `ReviewInput`
- syncing standalone artifacts
- syncing GitHub draft reviews
- reading/updating pending draft reviews
- cleaning up retained checkouts

The existing GitHub-facing types can become thin adapters over this module or be replaced at call sites where a cleaner entry point is better.

### Seams and Adapters

Keep the existing outer seams:

- `ReviewSource`
- `ArtifactSyncDestination`

Behind those seams, the GitHub review workflow owns internal seams for:

- GitHub command execution via `gh`
- git checkout operations via `git`
- filesystem interactions for checkout lifecycle

This becomes a real seam with two adapters:

1. **Production adapter** — runs `gh`/`git`, touches the filesystem, parses real command responses
2. **Test adapter** — returns deterministic discovery, pull-request, sync, and checkout results without spawning processes

### Data Flow

#### Discovery

The workflow loads candidate pull requests from GitHub, applies scoped discovery rules, resolves head SHAs, and yields `Review request` values for the runtime.

#### Preparation

The workflow loads pull-request details and diff content, ensures the correct checkout state, then returns one prepared `ReviewInput`.

Checkout lifecycle is implementation detail, not caller knowledge.

#### Sync

The workflow accepts local artifacts and routes them to the right GitHub destination:

- dry-run sync state
- PR comments
- draft review summary/comment sync
- pending review read/update operations

#### Cleanup

The workflow enumerates retained checkouts, validates whether they still belong to allowed repositories and active pull requests, and removes the ones that no longer qualify.

## Error Handling

The interface returns existing `AgentError` values, but low-level command, parsing, rate-limit, and filesystem behavior is mapped in one place.

This keeps current error detail while improving **locality**:

- GitHub CLI failure mapping stays concentrated
- rate-limit detection stays concentrated
- checkout/path failure handling stays concentrated
- callers stop owning partial workflow recovery logic

## Testing Strategy

The interface is the test surface.

High-value tests should cover:

- discovery with and without allowlist scopes
- already-reviewed checks
- preparation of `ReviewInput`, including checkout decisions
- standalone artifact sync
- batch draft-review sync
- pending review fetch/update behavior
- checkout cleanup behavior
- GitHub rate-limit and command-failure mapping

The production adapter may still need a small set of focused adapter tests around command arguments and response parsing. Existing helper-level tests should shrink where workflow-level tests now provide the better seam.

## Migration Plan

1. Introduce the deep GitHub review workflow implementation and its internal adapter traits
2. Move discovery, preparation, sync, cleanup, and error mapping behavior behind that module
3. Rewire current public GitHub types or workspace call sites to delegate to the new workflow
4. Replace helper-centric tests with workflow-level tests where coverage overlaps
5. Remove redundant helpers and narrow the remaining public surface

## Expected Benefits

### Locality

- One place to change GitHub pull-request behavior
- One place to debug checkout, sync, and rate-limit issues
- Less cross-file hopping to understand the GitHub review flow

### Leverage

- Callers learn one workflow interface instead of several partially-overlapping types
- Tests exercise more behavior per entry point
- Future GitHub features fit behind the same seam instead of adding more top-level helpers

## Open Decisions Already Resolved

- The module name is **GitHub review workflow**
- The module owns the full GitHub workflow: discovery, preparation, sync, and cleanup
- Checkout management stays behind the module interface
- The refactor may change call sites in other crates to get a cleaner interface
- `ReviewSource` and `ArtifactSyncDestination` remain the outer seams
