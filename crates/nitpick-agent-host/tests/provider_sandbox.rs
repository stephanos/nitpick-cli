#![cfg(target_os = "macos")]

use std::{env, fs, process::Command};

use nitpick_agent_core::{AgentProviderKind, CommandAgentProvider, CommandSandboxConfig};

#[test]
fn sandboxed_claude_can_read_and_refresh_anthropic_credentials() {
    const CHILD_ENV: &str = "NITPICK_TEST_ANTHROPIC_SANDBOX_CHILD";
    if env::var_os(CHILD_ENV).is_none() {
        let home = tempfile::tempdir_in(env::var_os("HOME").expect("HOME")).expect("home");
        let data = tempfile::tempdir().expect("data");
        let credentials = home.path().join(".config/anthropic/credentials");
        fs::create_dir_all(&credentials).expect("credentials directory");
        fs::write(credentials.join("default.json"), "original").expect("credentials");
        fs::create_dir_all(home.path().join(".config/unrelated")).expect("unrelated directory");
        fs::write(home.path().join(".config/unrelated/secret"), "private").expect("unrelated file");

        let output = Command::new(env::current_exe().expect("test executable"))
            .args([
                "--exact",
                "sandboxed_claude_can_read_and_refresh_anthropic_credentials",
                "--nocapture",
            ])
            .env(CHILD_ENV, "1")
            .env("HOME", home.path())
            .env("NITPICK_AGENT_DATA_DIR", data.path())
            .output()
            .expect("child test");
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            fs::read_to_string(credentials.join("default.json")).expect("refreshed credentials"),
            "refreshed"
        );
        return;
    }

    let repo = tempfile::tempdir().expect("repo");
    let provider = CommandAgentProvider::new(AgentProviderKind::Claude, None, "/bin/sh")
        .with_sandbox(
            CommandSandboxConfig::nono()
                .with_helper_command(env!("CARGO_BIN_EXE_nitpick-agent-host"))
                .without_nono_profile_updates(),
        );
    let script = r#"
set -eu
config="$HOME/.config/anthropic"
test "$(cat "$config/credentials/default.json")" = original
printf locked > "$config/credentials.lock"
printf refreshed > "$config/credentials/default.json.tmp"
mv "$config/credentials/default.json.tmp" "$config/credentials/default.json"
rm "$config/credentials.lock"
if cat "$HOME/.config/unrelated/secret" >/dev/null 2>&1; then
    exit 2
fi
if (printf forbidden > "$HOME/.config/unrelated/secret") 2>/dev/null; then
    exit 2
fi
"#;
    let output = provider
        .command_for_testing(Some(repo.path()), &["-c".into(), script.into()])
        .expect("sandboxed command")
        .current_dir(repo.path())
        .output()
        .expect("provider output");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
