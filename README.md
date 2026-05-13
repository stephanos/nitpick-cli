# nitpick-agent

Reusable agent runtime for Nitpick-style code review workflows.

This project is intended to become:

- a Rust library that Nitpick can import for provider/session/review-agent behavior
- a standalone CLI for using the same agent runtime outside the Nitpick app
- a local host daemon that owns shared activity state for CLI and desktop clients
- a macOS menu bar `.app` with Sparkle update checks

Nitpick should continue to own GitHub sync, dashboard state, app lifecycle, and UI. This crate should stay focused on generic agent concerns: provider execution, session continuity, review prompts, structured review output, and chat.

## Storage Model

Review results, review comments, summaries, chat responses, sessions, and activity metadata are local artifacts first. Local storage is the source of truth; GitHub is an outbound sync destination, not the authoritative store.

By default, the host reads config from:

```text
~/.config/nitpick-agent/config.toml
```

and stores local source-of-truth data under:

```text
~/.local/share/nitpick-agent
```

Override these with `NITPICK_AGENT_CONFIG` and `NITPICK_AGENT_DATA_DIR`.
GitHub PR checkouts are retained under the data directory at `checkouts/` by default, and can be moved with `NITPICK_AGENT_CHECKOUT_DIR`.
When the macOS app starts the host daemon, stdout/stderr are appended to `logs/daemon.log` under the nitpick-agent data directory. By default this is `~/.local/share/nitpick-agent/logs/daemon.log`.

The host API listens on `127.0.0.1:19783` by default when started with:

```bash
nitpick-agent-host daemon
```

Override the bind address with `NITPICK_AGENT_HOST_ADDR`.

The CLI reads host status from the same local API:

```bash
nitpick status
nitpick review acme/platform#42
nitpick inspect acme/platform#42
nitpick review-requests
nitpick review-requests --new
nitpick chat "summarize this repo"
nitpick reviews
nitpick reviews --all
nitpick logs activity-1
nitpick logs acme/platform#42
nitpick logs daemon
nitpick resume activity-1
nitpick resume acme/platform#42
nitpick review-sync activity-1 acme/platform#42
nitpick activities
nitpick artifacts activity-1
nitpick artifact artifact-1
nitpick artifact-sync artifact-1 github
nitpick artifact-sync artifact-1 github-review acme/platform#42
nitpick artifact-sync artifact-1 github acme/platform#42
nitpick sync-pending github
nitpick cleanup-checkouts
```

The daemon can watch review sources and create local review activities automatically. GitHub is the first source adapter; additional source-code providers should plug into the same review-source API. Processed review heads are stored locally, so a review request is not reviewed again until its head SHA changes.

```toml
[sources.github.discovery]
enabled = true
auto_review = true
interval_seconds = 300
```

The older `[github.discovery]` config shape is still accepted for compatibility.

`artifact-sync ... github` without a target uses the GitHub dry-run destination and records the local artifact as pending sync. Provide a target such as `acme/platform#42` to post through `gh pr comment`; the local artifact is then marked synced with the returned comment URL/text. Use `github-review` with a target to sync one review artifact. Use `review-sync <activity-id> <pr-ref>` to post all review artifacts from an activity as one GitHub pull request review.

Agent execution is handled by external commands. By default `provider = "claude"` runs `claude` and `provider = "codex"` runs `codex`; override the executable path with `command` in the config file. PR reviews get stable local provider session IDs; Claude receives them with `--session-id`, while Codex currently keeps the ID in local state only. Stored Claude and Codex sessions can be reopened with `nitpick resume <activity-id|pr-ref>` when the activity has a provider session ID. GitHub posting uses `gh` by default; override it with `github_command`.

## Layout

```text
crates/nitpick-agent-core    generic review/chat runtime
crates/nitpick-agent-cli     terminal entry point
crates/nitpick-agent-client  Rust client for the local host API
crates/nitpick-agent-host    local daemon process
crates/nitpick-agent-github  GitHub adapter helpers
crates/nitpick-agent-integration-tests
                              host-level integration tests with stubs
macos/                       Swift menu bar app and Sparkle packaging
```

## Current Status

This is a scaffold. The core runtime now has the first activity/session/store boundary, command-based provider execution, local JSON-backed artifact storage, schema-versioned store metadata, GitHub review-request discovery, GitHub posting via `gh`, and a host API for status, activities, artifacts, asynchronous review submission, and asynchronous chat submission. The macOS app shell can build with Sparkle.

## Commands

```bash
mise run setup
mise run test
mise run test-macos
mise run build
mise run macos-app
mise run macos-appcast
mise run install
mise run verify
mise exec -- cargo run -p nitpick-agent-cli --bin nitpick -- --help
```

`mise run macos-app` writes `target/macos/Nitpick Agent.app`. `mise run macos-appcast` signs a Sparkle appcast with the private EdDSA key from Keychain account `nitpick-agent` locally. In GitHub Actions, the release workflow reads the private key from the repository secret `SPARKLE_PRIVATE_ED_KEY`.

`mise run install` installs `Nitpick Agent.app` into `/Applications` and launches it. When the app starts, it installs the bundled CLI as `~/.local/bin/nitpick`.

## GitHub CI

The repository has two GitHub Actions workflows:

- `test`: runs `mise run verify` on pushes and pull requests.
- `release`: runs on `v*` tags, builds the signed app archive, generates the Sparkle appcast, and creates a GitHub release.

Release signing expects these GitHub secrets:

- `CODESIGN_IDENTITY`: signing identity passed to `codesign`.
- `SPARKLE_PRIVATE_ED_KEY`: private Sparkle EdDSA key stored as a GitHub repository secret for appcast signing.
