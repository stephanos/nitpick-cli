use nitpick_agent_host::{AgentConfig, REVIEW_PROMPT_TEMPLATE};

#[test]
fn init_review_prompt_file_creates_example_next_to_prompt() {
    let dir = tempfile::tempdir().expect("temp dir");
    let config_path = dir.path().join("nested/config.toml");

    AgentConfig::init_review_prompt_file(&config_path).expect("init review prompt");

    assert_eq!(
        std::fs::read_to_string(dir.path().join("nested/review-prompt.example.md"))
            .expect("review prompt example"),
        REVIEW_PROMPT_TEMPLATE
    );
}

#[test]
fn init_review_prompt_file_overwrites_existing_example() {
    let dir = tempfile::tempdir().expect("temp dir");
    let config_path = dir.path().join("config.toml");
    let prompt_path = dir.path().join("review-prompt.md");
    let example_path = dir.path().join("review-prompt.example.md");
    std::fs::write(&prompt_path, "custom prompt").expect("write custom prompt");
    std::fs::write(&example_path, "old example").expect("write old example");

    AgentConfig::init_review_prompt_file(&config_path).expect("init review prompt");

    assert_eq!(
        std::fs::read_to_string(example_path).expect("review prompt example"),
        REVIEW_PROMPT_TEMPLATE
    );
    assert_eq!(
        std::fs::read_to_string(prompt_path).expect("review prompt"),
        "custom prompt"
    );
}
