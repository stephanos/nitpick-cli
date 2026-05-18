# nitpick-agent-cli clap command structure design

## Problem

`crates/nitpick-agent-cli` already uses `clap`, but the crate still concentrates CLI parsing, command definitions, dispatch, formatting, and tests inside `src/lib.rs`. The public CLI is also mostly a flat list of commands. That makes the command surface harder to extend and keeps unrelated responsibilities coupled in one file.

This design restructures the crate around domain-oriented command groups, lets `clap` model the new nested command tree directly, and splits parser and handler code into files organized by command area.

## Goals

- Use `clap` as the source of truth for the CLI hierarchy.
- Replace the current flat command surface with nested domain subcommands.
- Organize source files by command area instead of keeping one large `lib.rs`.
- Preserve runtime behavior behind the commands where practical: same host calls, same sandbox behavior, same output formatting, and comparable error messages.
- Update documentation and tests to match the new CLI surface.

## Non-goals

- Keep backward-compatible aliases for the old flat commands.
- Redesign host APIs or change the underlying activity, artifact, review, or chat behavior.
- Introduce a broad abstraction layer that hides command-specific behavior.

## Proposed CLI hierarchy

The new public shape is domain-first:

```text
nitpick system status
nitpick system cleanup-checkouts

nitpick review run <pr-ref>
nitpick review requests [--new]
nitpick review sync <activity-id> <pr-ref>
nitpick review list [--all]

nitpick artifact list <activity-id>
nitpick artifact show <artifact-id>
nitpick artifact sync <artifact-id> <destination> [target]

nitpick activity list
nitpick activity logs <activity-id|pr-ref|daemon>
nitpick activity resume <activity-id|pr-ref>
nitpick activity inspect <pr-ref>

nitpick chat start <pr-ref>
```

### Rationale

- `review`, `artifact`, and `activity` become clear namespaces that group related behavior.
- `system` holds operational commands that do not belong to one review artifact or activity.
- `chat` becomes explicit about the action being performed rather than standing alone as a special-case top-level command.

## Module layout

The crate should be split so parser definitions and runtime handlers mirror the CLI hierarchy:

```text
crates/nitpick-agent-cli/src/
  main.rs
  lib.rs
  cli/
    mod.rs
    review.rs
    artifact.rs
    activity.rs
    system.rs
    chat.rs
  commands/
    mod.rs
    review.rs
    artifact.rs
    activity.rs
    system.rs
    chat.rs
  output/
    mod.rs
    activity.rs
    artifact.rs
    review.rs
    status.rs
  context.rs
```

### Responsibility split

- `cli/*`: `clap` parser structs and enums only.
- `commands/*`: command execution for one domain group at a time.
- `output/*`: formatting helpers for status, activities, artifacts, reviews, and logs.
- `context.rs`: shared runtime inputs such as host address, repo directory, diff/context strings, config path, and data directory.
- `lib.rs`: public exports and a small root runner that connects parsing to dispatch.

This keeps each file focused and avoids recreating a single giant coordination module under a different name.

## Parsing and dispatch flow

Parsing should become a typed tree rooted in `clap`:

1. `Cli` parses global flags and a top-level `CommandGroup`.
2. Each group owns its own subcommand enum, such as `ReviewCommand` or `ActivityCommand`.
3. The root runner dispatches by domain group.
4. The matching `commands/*` module handles the leaf command with `CliRunContext` and `CliOptions`.

The design deliberately avoids converting everything back into one large flat enum before dispatch. Keeping domain enums intact makes new subcommands local changes and reduces the size of any single match expression.

## Shared behavior

The following logic remains shared across command groups:

- host client construction,
- config path and data directory resolution,
- sandbox option application,
- cached checkout lookup,
- provider session recovery and cleanup,
- reusable output formatting.

Each handler returns `Result<String, CliError>`. The binary continues to print the message and exit non-zero on failure. Errors stay explicit and command-specific rather than being swallowed by a generic wrapper.

## Migration plan for the CLI surface

This refactor is a deliberate breaking CLI change:

- Old flat commands are removed rather than preserved as aliases.
- README examples and any command documentation are updated to the new nested hierarchy.
- Parser tests are rewritten to assert the new command paths.

Behavior beneath the command paths should stay stable unless the nesting itself requires small wording changes in help output or parser diagnostics.

## Testing strategy

Tests should follow the new boundaries:

- parser tests near `cli/*` modules,
- domain execution tests near `commands/*` modules where practical,
- a small top-level smoke test for `--help`, version output, and representative nested command parsing.

The focus is to verify both the new public CLI hierarchy and unchanged runtime behavior behind it.

## Risks and mitigations

### Risk: command discoverability changes

Nested commands may surprise existing users.

**Mitigation:** keep help text concise and domain-oriented so `nitpick <group> --help` is easy to navigate, and update README examples in the same change.

### Risk: refactor accidentally changes behavior

Moving code out of `lib.rs` can introduce subtle regressions.

**Mitigation:** keep shared helpers centralized, move formatters without changing their behavior, and update parser and runner tests as the module split happens.

## Accepted design decisions

- Use full domain subcommands rather than a mixed namespace/action model.
- Replace the flat command surface outright instead of adding compatibility aliases.
- Organize files by domain groups with shared handlers in each group.
- Keep runtime behavior stable while treating the CLI shape itself as the intended breaking change.
