use crate::{ChatInput, ReviewInput};

pub enum ReviewPromptOutput {
    JsonFile { output_path: String },
    Tools { tool_instructions: String },
}

pub fn render_review_prompt(
    model: Option<&str>,
    input: &ReviewInput,
    output: ReviewPromptOutput,
) -> String {
    match output {
        ReviewPromptOutput::JsonFile { output_path } => format!(
            "{}\n\nmodel: {}\nrepository: {}\nnumber: {}\ntitle: {}\nauthor: {}\nrepo_dir: {}\ninstructions:\n{}\n\ndiff:\n{}\n",
            initial_review_prompt(input, &output_path),
            model.unwrap_or("(default)"),
            input.subject.repository,
            input
                .subject
                .number
                .map(|number| number.to_string())
                .unwrap_or_else(|| "(none)".into()),
            input.subject.title,
            input.subject.author,
            input.repo_dir.display(),
            input.instructions,
            input.diff,
        ),
        ReviewPromptOutput::Tools { tool_instructions } => format!(
            "{}\n\nmodel: {}\nrepository: {}\nnumber: {}\ntitle: {}\nauthor: {}\nrepo_dir: {}\ntool instructions:\n{}\n\ninstructions:\n{}\n\ndiff:\n{}\n",
            initial_review_tool_prompt(input),
            model.unwrap_or("(default)"),
            input.subject.repository,
            input
                .subject
                .number
                .map(|number| number.to_string())
                .unwrap_or_else(|| "(none)".into()),
            input.subject.title,
            input.subject.author,
            input.repo_dir.display(),
            tool_instructions,
            input.instructions,
            input.diff,
        ),
    }
}

pub(crate) fn render_chat_prompt(model: Option<&str>, input: &ChatInput) -> String {
    format!(
        "You are answering a development question.\n\nmodel: {}\nrepo_dir: {}\ncontext:\n{}\n\nprompt:\n{}\n",
        model.unwrap_or("(default)"),
        input.repo_dir.display(),
        input.context,
        input.prompt,
    )
}

fn initial_review_prompt(input: &ReviewInput, output_path: &str) -> String {
    let prompt = input.review_prompt.trim();
    let prompt = if prompt.is_empty() {
        include_str!("../../../examples/review-prompt.md")
    } else {
        prompt
    };
    prompt.replace("{review_output_path}", output_path)
}

fn initial_review_tool_prompt(input: &ReviewInput) -> String {
    let prompt = input.review_prompt.trim();
    let prompt = if prompt.is_empty() {
        include_str!("../../../examples/review-prompt.md")
    } else {
        prompt
    };
    let prompt = prompt.replace(
        "Write review annotations as JSON to `{review_output_path}` relative to the repository root. Do not return review annotations on stdout.",
        "Record review annotations with the Nitpick review MCP tools. Do not write review annotations to stdout or to a file.",
    );
    let prompt = prompt.replace(
        "The JSON object must contain `comments`. Each comment must use a repository-relative path, a line number inside the diff changeset, and a body. Use line 0 only for file-level comments on files in the diff changeset.",
        "Each comment must use a repository-relative path, a line number inside the diff changeset, and a body. Use line 0 only for file-level comments on files in the diff changeset.",
    );
    prompt.replace("{review_output_path}", "the Nitpick review MCP tools")
}
