use crate::{AgentMessage, AgentSession};

pub const MAX_PROVIDER_LOG_BYTES: usize = 64 * 1024;

pub fn push_provider_log(session: &mut AgentSession, role: &str, bytes: &[u8]) {
    let content = bounded_provider_log(bytes);
    if content.is_empty() {
        return;
    }
    session.messages.push(AgentMessage {
        role: role.into(),
        content,
    });
}

pub fn upsert_provider_log(session: &mut AgentSession, role: &str, content: &str) {
    if let Some(message) = session
        .messages
        .iter_mut()
        .find(|message| message.role == role)
    {
        message.content = content.into();
    } else {
        session.messages.push(AgentMessage {
            role: role.into(),
            content: content.into(),
        });
    }
}

pub fn bounded_provider_log(bytes: &[u8]) -> String {
    let truncated = bytes.len() > MAX_PROVIDER_LOG_BYTES;
    let start = bytes.len().saturating_sub(MAX_PROVIDER_LOG_BYTES);
    let mut value = String::from_utf8_lossy(&bytes[start..]).trim().to_owned();
    if truncated {
        value = format!("[truncated to last {MAX_PROVIDER_LOG_BYTES} bytes]\n{value}");
    }
    value
}

pub fn is_provider_log_role(role: &str) -> bool {
    matches!(
        role,
        "provider.stdout" | "provider.stderr" | "provider.sandbox"
    )
}
