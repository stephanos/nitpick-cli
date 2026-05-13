# Daemon Log Persistence Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. Do not create a git worktree for this repo, and do not commit unless the user explicitly asks.

**Goal:** Persist daemon stdout/stderr to a predictable log file and expose it through `nitpick logs daemon`.

**Architecture:** Add shared path helpers for the data directory and daemon log file in the CLI crate, and mirror the same path logic in the macOS core module. The macOS launcher redirects the bundled host process stdout/stderr to `logs/daemon.log`; the CLI reads that file when the user runs `nitpick logs daemon`, while existing `nitpick logs <activity-id|pr-ref>` behavior remains unchanged.

**Tech Stack:** Rust workspace, Swift Package under `macos/`, existing CLI/host process code, Cargo tests, Swift tests via `mise run test-macos`.

---

## File Structure

- Modify `crates/nitpick-agent-cli/src/lib.rs`
  - Add data-dir and daemon-log path helpers.
  - Special-case `logs daemon`.
  - Add a daemon log formatter that reports a clear not-found message.
- Modify `crates/nitpick-agent-cli/src/main.rs`
  - Pass the resolved data dir into `run_cli_command`.
- Modify `crates/nitpick-agent-integration-tests/tests/cli_smoke.rs`
  - Pass the new data-dir argument to existing CLI smoke calls.
  - Smoke-test `nitpick logs daemon` against a real temp log file.
- Modify `macos/Sources/NitpickAgentMacOSCore/ConfigFile.swift`
  - Add `DataDirectory` and `DaemonLogFile` path helpers next to config path logic.
- Modify `macos/Tests/NitpickAgentMacOSCoreTests/ConfigFileTests.swift`
  - Cover explicit, XDG, and default data-dir paths plus daemon log path.
- Modify `macos/Sources/NitpickAgentApp/HostProcess.swift`
  - Redirect stdout and stderr to `logs/daemon.log` instead of unretained pipes.
- Modify `README.md`
  - Document `nitpick logs daemon` and the default log path.
- Modify `plan.md`
  - Mark daemon logs as covered; remaining gap is Codex attach/resume.

## Task 1: Add CLI Data-Dir And Daemon Log Helpers

**Files:**
- Modify: `crates/nitpick-agent-cli/src/lib.rs`

- [ ] **Step 1: Write failing tests for path helpers and daemon log reading**

Add these tests inside the existing `#[cfg(test)] mod tests` in `crates/nitpick-agent-cli/src/lib.rs`:

```rust
#[test]
fn resolves_data_dir_like_host() {
    assert_eq!(
        super::data_dir_from_env(Some("/tmp/data".into()), None, None),
        std::path::PathBuf::from("/tmp/data")
    );
    assert_eq!(
        super::data_dir_from_env(None, Some("/tmp/xdg-data".into()), None),
        std::path::PathBuf::from("/tmp/xdg-data/nitpick-agent")
    );
    assert_eq!(
        super::data_dir_from_env(None, None, Some("/Users/stephan".into())),
        std::path::PathBuf::from("/Users/stephan/.local/share/nitpick-agent")
    );
}

#[test]
fn daemon_log_path_lives_under_data_dir() {
    assert_eq!(
        super::daemon_log_path(&std::path::PathBuf::from("/tmp/nitpick-data")),
        std::path::PathBuf::from("/tmp/nitpick-data/logs/daemon.log")
    );
}

#[test]
fn formats_missing_daemon_log_with_path() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("logs/daemon.log");

    let output = super::format_daemon_log(&path).expect("daemon log");

    assert_eq!(
        output,
        format!(
            "daemon log not found: {}\nrestart the macOS app or host after updating to log persistence",
            path.display()
        )
    );
}

#[test]
fn formats_existing_daemon_log() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("logs/daemon.log");
    std::fs::create_dir_all(path.parent().expect("parent")).expect("log dir");
    std::fs::write(&path, "started\npoll failed\n").expect("write log");

    let output = super::format_daemon_log(&path).expect("daemon log");

    assert_eq!(output, "started\npoll failed\n");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
cargo test -p nitpick-agent-cli daemon_log
```

Expected: compile failure because `data_dir_from_env`, `daemon_log_path`, and `format_daemon_log` do not exist yet.

- [ ] **Step 3: Implement CLI helpers**

Add these functions near `config_path_from_env` in `crates/nitpick-agent-cli/src/lib.rs`:

```rust
pub fn data_dir_from_env(
    nitpick_agent_data_dir: Option<std::ffi::OsString>,
    xdg_data_home: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
) -> std::path::PathBuf {
    if let Some(path) = nitpick_agent_data_dir {
        return std::path::PathBuf::from(path);
    }
    if let Some(data_home) = xdg_data_home {
        return std::path::PathBuf::from(data_home).join("nitpick-agent");
    }
    std::path::PathBuf::from(home.unwrap_or_else(|| ".".into()))
        .join(".local")
        .join("share")
        .join("nitpick-agent")
}

pub fn daemon_log_path(data_dir: &std::path::Path) -> std::path::PathBuf {
    data_dir.join("logs").join("daemon.log")
}

pub fn format_daemon_log(path: &std::path::Path) -> Result<String, String> {
    match std::fs::read_to_string(path) {
        Ok(contents) => Ok(contents),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(format!(
            "daemon log not found: {}\nrestart the macOS app or host after updating to log persistence",
            path.display()
        )),
        Err(error) => Err(format!("read daemon log {}: {error}", path.display())),
    }
}
```

- [ ] **Step 4: Run focused tests**

Run:

```bash
cargo test -p nitpick-agent-cli daemon_log
```

Expected: all daemon-log helper tests pass.

## Task 2: Extend `nitpick logs` With `daemon`

**Files:**
- Modify: `crates/nitpick-agent-cli/src/lib.rs`
- Modify: `crates/nitpick-agent-cli/src/main.rs`

- [ ] **Step 1: Write failing CLI behavior test**

Add this test inside `crates/nitpick-agent-cli/src/lib.rs`:

```rust
#[test]
fn logs_daemon_reads_daemon_log_without_host_lookup() {
    let dir = tempfile::tempdir().expect("temp dir");
    let data_dir = dir.path().join("data");
    let path = super::daemon_log_path(&data_dir);
    std::fs::create_dir_all(path.parent().expect("parent")).expect("log dir");
    std::fs::write(&path, "daemon started\n").expect("write log");

    let output = super::run_cli_command(
        CliCommand::Logs {
            target: "daemon".into(),
        },
        "127.0.0.1:1",
        dir.path().to_path_buf(),
        String::new(),
        String::new(),
        dir.path().join("config.toml"),
        data_dir,
    )
    .expect("logs daemon");

    assert_eq!(output, "daemon started\n");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
cargo test -p nitpick-agent-cli logs_daemon_reads_daemon_log_without_host_lookup
```

Expected: compile failure because `run_cli_command` does not accept `data_dir`, or runtime failure because `logs daemon` still tries to resolve an activity.

- [ ] **Step 3: Add `data_dir` to `run_cli_command`**

Change `run_cli_command` in `crates/nitpick-agent-cli/src/lib.rs` to:

```rust
pub fn run_cli_command(
    command: CliCommand,
    host_addr: &str,
    repo_dir: std::path::PathBuf,
    diff: String,
    context: String,
    config_path: std::path::PathBuf,
    data_dir: std::path::PathBuf,
) -> Result<String, String>
```

In the `CliCommand::Logs` arm, add the special case before host activity lookup:

```rust
        CliCommand::Logs { target } if target == "daemon" => {
            format_daemon_log(&daemon_log_path(&data_dir))
        }
```

Keep the existing activity/PR log arm after it:

```rust
        CliCommand::Logs { target } => {
            let activities = client.activities()?;
            let activity = resolve_log_activity(&activities, &target)?;
            let artifacts = client.activity_artifacts(activity.id.as_str())?;
            Ok(format_activity_logs(activity, &artifacts))
        }
```

- [ ] **Step 4: Update `main.rs` to pass data dir**

In `crates/nitpick-agent-cli/src/main.rs`, import `data_dir_from_env` and compute the data dir:

```rust
use nitpick_agent_cli::{
    config_path_from_env, data_dir_from_env, host_addr_from_env, parse_command, run_cli_command,
};
```

Inside `run()`:

```rust
    let data_dir = data_dir_from_env(
        env::var_os("NITPICK_AGENT_DATA_DIR"),
        env::var_os("XDG_DATA_HOME"),
        env::var_os("HOME"),
    );
    let output = run_cli_command(command, &addr, repo_dir, diff, context, config_path, data_dir)?;
```

- [ ] **Step 5: Run focused CLI test**

Run:

```bash
cargo test -p nitpick-agent-cli logs_daemon_reads_daemon_log_without_host_lookup
```

Expected: pass.

## Task 3: Update Integration Smoke For Data Dir Argument

**Files:**
- Modify: `crates/nitpick-agent-integration-tests/tests/cli_smoke.rs`

- [ ] **Step 1: Update existing calls and add daemon log smoke**

In `cli_commands_talk_to_the_host_api`, create:

```rust
let data_dir = temp.path().join("data");
let daemon_log = data_dir.join("logs/daemon.log");
std::fs::create_dir_all(daemon_log.parent().expect("daemon log parent"))
    .expect("daemon log dir");
std::fs::write(&daemon_log, "daemon started\n").expect("daemon log");
```

Pass `data_dir.clone()` as the final argument to every `run_cli_command` call.

After the existing activity log assertions, add:

```rust
let daemon_logs = run_cli_command(
    CliCommand::Logs {
        target: "daemon".into(),
    },
    &host_addr,
    repo_dir.clone(),
    String::new(),
    String::new(),
    config_path.clone(),
    data_dir.clone(),
)
.expect("daemon logs command");
assert_eq!(daemon_logs, "daemon started\n");
```

- [ ] **Step 2: Run integration smoke**

Run:

```bash
cargo test -p nitpick-agent-integration-tests cli_commands_talk_to_the_host_api
```

Expected: pass.

## Task 4: Add macOS Data And Log Path Helpers

**Files:**
- Modify: `macos/Sources/NitpickAgentMacOSCore/ConfigFile.swift`
- Modify: `macos/Tests/NitpickAgentMacOSCoreTests/ConfigFileTests.swift`

- [ ] **Step 1: Write failing Swift tests**

Add these tests to `ConfigFileTests`:

```swift
func testDataDirectoryUsesExplicitEnvironmentPath() {
    let dataDirectory = DataDirectory(
        environment: ["NITPICK_AGENT_DATA_DIR": "/tmp/nitpick-data"],
        homeDirectoryURL: URL(fileURLWithPath: "/Users/test")
    )

    XCTAssertEqual(dataDirectory.url.path, "/tmp/nitpick-data")
}

func testDataDirectoryUsesXdgDataHomeWhenPresent() {
    let dataDirectory = DataDirectory(
        environment: ["XDG_DATA_HOME": "/tmp/data"],
        homeDirectoryURL: URL(fileURLWithPath: "/Users/test")
    )

    XCTAssertEqual(dataDirectory.url.path, "/tmp/data/nitpick-agent")
}

func testDataDirectoryDefaultsToHomeLocalSharePath() {
    let dataDirectory = DataDirectory(
        environment: [:],
        homeDirectoryURL: URL(fileURLWithPath: "/Users/test")
    )

    XCTAssertEqual(dataDirectory.url.path, "/Users/test/.local/share/nitpick-agent")
}

func testDaemonLogFileLivesUnderDataDirectory() {
    let dataDirectory = DataDirectory(
        environment: ["NITPICK_AGENT_DATA_DIR": "/tmp/nitpick-data"],
        homeDirectoryURL: URL(fileURLWithPath: "/Users/test")
    )

    let logFile = DaemonLogFile(dataDirectory: dataDirectory)

    XCTAssertEqual(logFile.url.path, "/tmp/nitpick-data/logs/daemon.log")
}
```

- [ ] **Step 2: Run Swift tests to verify failure**

Run:

```bash
mise run test-macos
```

Expected: compile failure because `DataDirectory` and `DaemonLogFile` do not exist.

- [ ] **Step 3: Add Swift path helpers**

Append to `macos/Sources/NitpickAgentMacOSCore/ConfigFile.swift`:

```swift
public struct DataDirectory {
    public let url: URL

    public init(
        environment: [String: String] = ProcessInfo.processInfo.environment,
        homeDirectoryURL: URL = FileManager.default.homeDirectoryForCurrentUser
    ) {
        if let configuredPath = environment["NITPICK_AGENT_DATA_DIR"], !configuredPath.isEmpty {
            url = URL(fileURLWithPath: configuredPath)
            return
        }

        if let dataHome = environment["XDG_DATA_HOME"], !dataHome.isEmpty {
            url = URL(fileURLWithPath: dataHome)
                .appendingPathComponent("nitpick-agent")
            return
        }

        url = homeDirectoryURL
            .appendingPathComponent(".local")
            .appendingPathComponent("share")
            .appendingPathComponent("nitpick-agent")
    }
}

public struct DaemonLogFile {
    public let url: URL

    public init(dataDirectory: DataDirectory = DataDirectory()) {
        url = dataDirectory.url
            .appendingPathComponent("logs")
            .appendingPathComponent("daemon.log")
    }
}
```

- [ ] **Step 4: Run Swift path tests**

Run:

```bash
mise run test-macos
```

Expected: pass or fail only on the later HostProcess logging behavior that has not been implemented yet.

## Task 5: Persist macOS Host Process Output

**Files:**
- Modify: `macos/Sources/NitpickAgentApp/HostProcess.swift`

- [ ] **Step 1: Change host process output setup**

Replace:

```swift
process.standardOutput = Pipe()
process.standardError = Pipe()
```

with:

```swift
let logURL = DaemonLogFile().url
do {
    try FileManager.default.createDirectory(
        at: logURL.deletingLastPathComponent(),
        withIntermediateDirectories: true
    )
    if !FileManager.default.fileExists(atPath: logURL.path) {
        FileManager.default.createFile(atPath: logURL.path, contents: nil)
    }
    let logHandle = try FileHandle(forWritingTo: logURL)
    try logHandle.seekToEnd()
    process.standardOutput = logHandle
    process.standardError = logHandle
} catch {
    process.standardOutput = Pipe()
    process.standardError = Pipe()
}
```

This appends to the existing log file and falls back to unpersisted pipes if the file cannot be opened.

- [ ] **Step 2: Run macOS tests**

Run:

```bash
mise run test-macos
```

Expected: pass.

## Task 6: Update Docs And Requirement Gap

**Files:**
- Modify: `README.md`
- Modify: `plan.md`

- [ ] **Step 1: Update README commands**

Add to the CLI command list:

```bash
nitpick logs daemon
```

Add a short note near the daemon startup section:

```text
When the macOS app starts the host daemon, stdout/stderr are appended to `logs/daemon.log` under the nitpick-agent data directory. By default this is `~/.local/share/nitpick-agent/logs/daemon.log`; override the data directory with `NITPICK_AGENT_DATA_DIR`.
```

- [ ] **Step 2: Update plan.md**

Change Requirement 8 current gap from:

```text
but not Codex attach/resume or daemon logs.
```

to:

```text
but not Codex attach/resume.
```

- [ ] **Step 3: Run whitespace check**

Run:

```bash
git diff --check
```

Expected: no whitespace errors.

## Task 7: Final Verification

**Files:**
- No file edits unless verification reveals failures.

- [ ] **Step 1: Format Rust**

Run:

```bash
cargo fmt --check
```

Expected: exit 0.

- [ ] **Step 2: Run Rust tests**

Run:

```bash
cargo test
```

Expected: all Rust tests pass.

- [ ] **Step 3: Run macOS tests**

Run:

```bash
mise run test-macos
```

Expected: all Swift tests pass.

- [ ] **Step 4: Diff hygiene and status**

Run:

```bash
git diff --check
git status --short
```

Expected: no whitespace errors. Status should show only planned files plus any already-existing unrelated untracked files.

## Self-Review

- Spec coverage: The plan covers persistent daemon output from the macOS-launched host, CLI discovery through `nitpick logs daemon`, path discoverability, docs, and requirement updates.
- Scope: Manual `nitpick-agent-host daemon` still logs to the terminal. That is intentional for this slice; the requested easy on-disk path is provided for the app-managed daemon and CLI reads the same file.
- Placeholder scan: No TODO/TBD placeholders remain.
- Type consistency: Rust helpers use `PathBuf`/`Path`; Swift helpers mirror existing `ConfigFile` style.
- Repository constraints: No git worktrees and no commit steps.
