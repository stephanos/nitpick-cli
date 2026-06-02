use nitpick_agent_core::{AgentProviderKind, ProviderRunTranscriptContext};
use std::{path::PathBuf, time::Duration};

#[test]
fn provider_run_transcript_start_records_runtime_context() {
    let transcript = ProviderRunTranscriptContext {
        provider: &AgentProviderKind::Claude,
        model: Some("claude-opus"),
        command: &PathBuf::from("/usr/bin/claude"),
        sandbox_enabled: true,
        timeout: Some(Duration::from_secs(30)),
        provider_debug_file: Some(&PathBuf::from("/tmp/provider.log")),
    };

    let diagnostic = transcript.start_diagnostic();

    assert!(diagnostic.contains("provider claude command running"));
    assert!(diagnostic.contains("model: claude-opus"));
    assert!(diagnostic.contains("sandbox: enabled"));
    assert!(diagnostic.contains("timeout: 30s"));
    assert!(diagnostic.contains("debug_file: /tmp/provider.log"));
}
