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

The host API listens on `127.0.0.1:19783` by default when started with:

```bash
nitpick-agent-host daemon
```

Override the bind address with `NITPICK_AGENT_HOST_ADDR`.

The CLI reads host status from the same local API:

```bash
nitpick-agent status
nitpick-agent review acme/platform#42
nitpick-agent review-requests
nitpick-agent review-requests --new
nitpick-agent chat "summarize this repo"
nitpick-agent activities
nitpick-agent artifacts activity-1
nitpick-agent artifact artifact-1
nitpick-agent artifact-sync artifact-1 github
nitpick-agent artifact-sync artifact-1 github acme/platform#42
nitpick-agent sync-pending github
```

The daemon can watch GitHub for pull requests requesting your review and create local review activities automatically. It stores processed PR heads locally, so a PR is not reviewed again until its head SHA changes.

```toml
[github.discovery]
enabled = true
auto_review = true
interval_seconds = 300
```

`artifact-sync ... github` without a target uses the GitHub dry-run destination and records the local artifact as pending sync. Provide a target such as `acme/platform#42` to post through `gh pr comment`; the local artifact is then marked synced with the returned comment URL/text.

Provider execution is handled by external commands. By default `provider = "claude"` runs `claude` and `provider = "codex"` runs `codex`; override the executable path with `command` in the config file. GitHub posting uses `gh` by default; override it with `github_command`.

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
mise exec -- cargo run -p nitpick-agent-cli -- --help
```

`mise run macos-app` writes `target/macos/Nitpick Agent.app`. `mise run macos-appcast` signs a Sparkle appcast with the private EdDSA key from Keychain account `nitpick-agent` locally. In GitHub Actions, the release workflow reads the private key from the repository secret `SPARKLE_PRIVATE_ED_KEY`.

`mise run install` installs `Nitpick Agent.app` into `/Applications` and launches it. When the app starts, it installs the bundled `nitpick-agent` CLI as `~/.local/bin/nitpick-agent`.

## GitHub CI

The repository has two GitHub Actions workflows:

- `test`: runs `mise run verify` on pushes and pull requests.
- `release`: runs on `v*` tags, builds the signed app archive, generates the Sparkle appcast, and creates a GitHub release.

Release signing expects these GitHub secrets:

- `CODESIGN_IDENTITY`: signing identity passed to `codesign`.
- `SPARKLE_PRIVATE_ED_KEY`: private Sparkle EdDSA key stored as a GitHub repository secret for appcast signing.
