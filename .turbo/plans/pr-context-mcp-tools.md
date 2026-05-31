---
status: draft
---

# Plan: PR Context MCP Tools

## Context

Nitpick review agents can currently record inline review comments through the `nitpick-review` MCP server and can read existing inline review comments with `existing_review_comments`. They are explicitly instructed to call that tool before reviewing. They do not currently receive the pull request description/body or general PR conversation comments through MCP, and GitHub discovery only fetches title, author, URL, head metadata, state, and diff.

The goal is to add review-session context tools so agents can inspect the PR description and PR conversation before reviewing, then update the prompt guidance to require using those tools. This should keep GitHub access centralized in Nitpick, avoid requiring the provider to run `gh` directly, and preserve the existing model where MCP tools are read-only for context and write-only for review output.

## Pattern Survey

### Analogous Features

- `crates/nitpick-agent-host/src/review_mcp.rs`
  - Existing MCP tool surface for review sessions: `existing_review_comments`, `delete_draft_comment`, `add_review_comment`, and `finish_review`.
  - Pattern: session state is serialized to a temp JSON file, tools read/update it through `ReviewMcpTools::from_state_path`, and direct in-memory sessions use `ActiveReviewSession`.
  - Alignment: add PR details and conversation data to `ReviewMcpSessionState`, expose them through read-only MCP tools, and mirror both Active and File session paths.
- `crates/nitpick-agent-host/src/lib.rs`
  - `HostReviewProvider` fetches existing GitHub review comments before starting the MCP server, then passes them into `ReviewMcpServerHandle::start`.
  - Pattern: best-effort GitHub context fetch logs a warning and falls back to empty context if GitHub access fails.
  - Alignment: add PR context fetches next to `existing_review_comments`, preserving best-effort behavior so review execution is not blocked by nonessential conversation context.
- `crates/nitpick-agent-github/src/lib.rs`
  - `GitHubCliReviewSyncDestination::review_comments` already fetches submitted inline comments, pending review comments, and pending draft comments through GitHub REST endpoints.
  - `GitHubCliDiscovery::review_input` already fetches PR details and diff through `gh pr view` and `gh pr diff`.
  - Alignment: extend the GitHub adapter with typed PR context fetchers rather than putting raw `gh` calls in host or MCP code.
- `examples/review-mcp-instructions.md`
  - Existing prompt-side contract tells agents to call `existing_review_comments` before reviewing and to finish with `finish_review`.
  - Alignment: update this same file to instruct agents to call the new context tools before reviewing.

### Reusable Utilities

- `GitHubCommand::json` and `GitHubCommand::json_with_start_error` in `crates/nitpick-agent-github/src/command.rs`
  - Reuse for `gh pr view --json body,...` and `gh api repos/{owner}/{repo}/issues/{number}/comments`.
- `PullRequestRef` in `crates/nitpick-agent-github/src/lib.rs`
  - Reuse as the common owner/repo/number carrier for new GitHub context fetchers.
- `pull_request_ref_from_review_input` in `crates/nitpick-agent-host/src/lib.rs`
  - Reuse to decide whether GitHub PR context tools are available for a review.
- `ReviewMcpServerHandle::start` and `ReviewMcpSessionState`
  - Extend the existing MCP state setup instead of introducing a second MCP server or provider-specific transport.
- `ReviewMcpTools::{new, from_state_path}`
  - Keep tests simple by using file-backed state for tool behavior and fake `gh` scripts for GitHub behavior.

### Convention Anchors

- Tool result structs derive `Clone`, `Debug`, `PartialEq`, `Eq`, `Serialize`, and `JsonSchema`; input structs derive `Deserialize` as needed.
- MCP tool methods have public synchronous helpers plus `#[tool]` async wrappers returning `Result<Json<T>, String>`.
- Existing host tests use fake `gh` shell scripts and command logs to assert exact GitHub calls.
- Existing command-provider tests assert the MCP config is passed to Claude/Codex and that prompt text includes tool instructions.
- Existing review output remains local-first: MCP tools collect local artifacts, and GitHub sync remains a later destination step.

### Proposed Alignment

Add a `PullRequestContext` model to MCP session state containing PR description/body and conversation comments. Fetch it in `HostReviewProvider` with a new GitHub adapter method, store it in the MCP session, expose it through read-only tools, and update review MCP instructions to make those tools part of the required pre-review workflow.

## Implementation Steps

1. **Add PR Context Models To MCP State**
   - Update `crates/nitpick-agent-host/src/review_mcp.rs`.
   - Add serializable MCP-facing structs, likely `PullRequestContext`, `PullRequestConversationComment`, `PullRequestContextResult`, and possibly `PullRequestDescriptionResult`.
   - Add fields to `ReviewMcpSessionState` with `#[serde(default)]` so older state files remain readable.
   - Add matching storage to `ActiveReviewSessionState` so `ReviewMcpTools::new` and direct unit tests can exercise the same behavior.

2. **Expose Read-Only MCP Tools**
   - In `ReviewMcpTools`, add synchronous helpers and `#[tool]` wrappers for:
     - `pull_request_context` or `pull_request_details`: returns title, author, URL if available, body/description, and head metadata if carried.
     - `pull_request_conversation_comments`: returns general PR conversation comments.
   - Keep `existing_review_comments` focused on inline review comments for backward compatibility.
   - Return empty/default context when the review is not GitHub-backed or context could not be fetched.

3. **Fetch PR Body And Conversation From GitHub**
   - Update `crates/nitpick-agent-github/src/lib.rs`.
   - Extend `PullRequestDetails` and `PullRequestDetailsResponse` to include `body`, using `gh pr view --json title,author,url,body,headRefOid,headRefName,state,mergedAt`.
   - Add typed models for PR conversation comments fetched from `gh api repos/{owner}/{repo}/issues/{number}/comments`.
   - Add a method on `GitHubCliReviewSyncDestination` or a small adjacent GitHub context helper that returns PR context for an existing `PullRequestRef`.
   - Preserve existing review-input behavior while making the new body field available to MCP context setup.

4. **Wire Context Into HostReviewProvider**
   - Update `crates/nitpick-agent-host/src/lib.rs`.
   - Add a `pull_request_context(&self, input: &ReviewInput)` helper next to `existing_review_comments`.
   - Pass the fetched context into `ReviewMcpServerHandle::start`.
   - Keep GitHub context fetch best-effort: log warnings and continue with empty context if `gh` fails, matching `existing_review_comments`.
   - Avoid exposing raw `gh` command access to the provider.

5. **Update Review Instructions And Prompt Coverage**
   - Update `examples/review-mcp-instructions.md`.
   - Require agents to call the PR context tool and conversation-comments tool before reviewing, alongside `existing_review_comments`.
   - Keep the instruction precise: use PR description and conversation comments to understand intent and avoid duplicate/outdated feedback; still use `add_review_comment` only for findings and `finish_review` exactly once.
   - Update `crates/nitpick-agent-core/tests/command_provider.rs` assertions so the prompt includes the new required pre-review tool calls.

6. **Add Tests For MCP Tool Behavior**
   - Extend `crates/nitpick-agent-host/tests/review_mcp.rs`.
   - Add file-backed state tests showing PR description/context and conversation comments are listed.
   - Add host-provider tests with fake `gh` responses for:
     - PR body from `gh pr view`.
     - issue comments from `gh api repos/{owner}/{repo}/issues/{number}/comments`.
     - graceful fallback when one context fetch fails.
   - Keep existing deletion and inline-comment tests unchanged.

7. **Add GitHub Adapter Tests**
   - Extend `crates/nitpick-agent-github/tests/discovery.rs` and/or `crates/nitpick-agent-github/tests/sync_destination.rs`.
   - Assert the exact `gh pr view` JSON field list includes `body`.
   - Assert PR conversation comments are parsed with id, author, body, created/updated timestamps if included, and URLs if included.
   - Add an empty-comments test so the MCP tool returns an empty list without error.

8. **Update Docs**
   - Update `README.md` around review command behavior and GitHub permissions.
   - Clarify that Nitpick gives review agents MCP access to PR description, PR conversation comments, existing inline review comments, and local review-output tools.
   - Note that conversation context is read-only and best-effort.

## Verification

- `cargo fmt --all`
  - Formatting succeeds.
- `cargo test -p nitpick-agent-host review_mcp`
  - MCP state/tool tests pass, including PR context and conversation comment tools.
- `cargo test -p nitpick-agent-github discovery sync_destination`
  - GitHub adapter tests pass for PR body and conversation comment fetches.
- `cargo test -p nitpick-agent-core command_provider`
  - Provider prompt tests confirm the new tool instructions are included for MCP-backed reviews.
- `cargo test --workspace`
  - Full workspace remains green.
- Manual smoke check:
  - Run a review against a fake or real GitHub PR through `nitpick review start owner/repo#number`.
  - Confirm provider logs show MCP config passed.
  - Confirm the MCP session state contains PR body/conversation comments before the provider finishes.
  - Confirm review still requires `finish_review` and still produces local review comments, not direct GitHub publication.

## Context Files

- `crates/nitpick-agent-host/src/review_mcp.rs` — MCP tool definitions, session state, and file-backed state update pattern.
- `crates/nitpick-agent-host/src/lib.rs` — `HostReviewProvider` wiring from review input to MCP handle and GitHub context fetches.
- `crates/nitpick-agent-github/src/lib.rs` — GitHub CLI adapter, PR metadata fetches, review comment fetches, and `PullRequestRef`.
- `crates/nitpick-agent-core/src/command_provider.rs` — MCP config provider args and prompt construction.
- `examples/review-mcp-instructions.md` — required agent behavior for MCP-backed reviews.
- `crates/nitpick-agent-host/tests/review_mcp.rs` — host/MCP behavior tests and fake provider patterns.
- `crates/nitpick-agent-github/tests/discovery.rs` — PR metadata and review input tests.
- `crates/nitpick-agent-github/tests/sync_destination.rs` — GitHub API fake-command tests for review comments.
- `crates/nitpick-agent-core/tests/command_provider.rs` — prompt and MCP argument tests for Claude/Codex providers.
