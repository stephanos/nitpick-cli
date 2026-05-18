use nitpick_agent_core::parse_json_str;

use crate::{HostClientError, HostClientResult};

pub(crate) fn parse_json<T: serde::de::DeserializeOwned>(body: &str) -> HostClientResult<T> {
    parse_json_str(body, "invalid host response").map_err(|error| match error {
        nitpick_agent_core::AgentError::Json { path, error, .. } => HostClientError::InvalidJson {
            path,
            message: error,
        },
        error => HostClientError::InvalidJson {
            path: "$".to_owned(),
            message: error.to_string(),
        },
    })
}

#[cfg(test)]
mod tests {
    use crate::{HostClientError, HostStatus};

    use super::parse_json;

    #[test]
    fn parse_json_reports_field_path() {
        let error = parse_json::<HostStatus>(r#"{"activity_count":"wrong"}"#)
            .expect_err("invalid field type");

        assert!(matches!(
            error,
            HostClientError::InvalidJson { path, .. } if path == "activity_count"
        ));
    }
}
