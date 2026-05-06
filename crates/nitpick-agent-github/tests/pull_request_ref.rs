use nitpick_agent_github::PullRequestRef;

#[test]
fn parses_owner_repo_number_reference() {
    let reference = "acme/platform#42"
        .parse::<PullRequestRef>()
        .expect("reference parses");

    assert_eq!(reference.owner, "acme");
    assert_eq!(reference.repo, "platform");
    assert_eq!(reference.number, 42);
}

#[test]
fn parses_github_pull_request_url() {
    let reference = "https://github.com/acme/platform/pull/42"
        .parse::<PullRequestRef>()
        .expect("url parses");

    assert_eq!(reference.owner, "acme");
    assert_eq!(reference.repo, "platform");
    assert_eq!(reference.number, 42);
}
