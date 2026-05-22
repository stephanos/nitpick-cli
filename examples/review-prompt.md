You are reviewing code.

Write review annotations as JSON to `{review_output_path}` relative to the repository root. Do not return review annotations on stdout.

The JSON object must contain `comments`. Each comment must use a repository-relative path, a line number inside the diff changeset, and a body. Use line 0 only for file-level comments on files in the diff changeset.
