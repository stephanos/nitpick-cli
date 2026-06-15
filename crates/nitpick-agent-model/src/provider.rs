use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum AgentProviderKind {
    #[default]
    Claude,
    Codex,
}

impl AgentProviderKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }
}

impl std::str::FromStr for AgentProviderKind {
    type Err = ParseAgentProviderKindError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "claude" => Ok(Self::Claude),
            "codex" => Ok(Self::Codex),
            value => Err(ParseAgentProviderKindError {
                value: value.to_owned(),
            }),
        }
    }
}

impl std::fmt::Display for AgentProviderKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl Serialize for AgentProviderKind {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for AgentProviderKind {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(D::Error::custom)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseAgentProviderKindError {
    value: String,
}

impl std::fmt::Display for ParseAgentProviderKindError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "unknown agent provider `{}`", self.value)
    }
}

impl std::error::Error for ParseAgentProviderKindError {}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderFailureKind {
    AuthInvalidCredentials,
    SandboxPermissionDenied,
    ProviderUnavailable,
    UnknownProviderFailure,
}

#[cfg(test)]
mod tests {
    use super::AgentProviderKind;

    #[test]
    fn serializes_provider_as_stable_lowercase_name() {
        let value = serde_json::to_string(&AgentProviderKind::Claude).expect("provider json");

        assert_eq!(value, r#""claude""#);
    }

    #[test]
    fn deserializes_legacy_provider_variant_name() {
        let value: AgentProviderKind = serde_json::from_str(r#""Claude""#).expect("provider json");

        assert_eq!(value, AgentProviderKind::Claude);
    }
}
