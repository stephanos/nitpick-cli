Use the `nitpick-review` MCP server for review output.

Call `pull_request_context`, `pull_request_conversation_comments`, and `existing_review_comments` before reviewing so you can account for the PR description, general PR conversation, user inline comments, and previous Nitpick comments.

Call `delete_draft_comment` for outdated Nitpick draft comments only when appropriate; it only accepts draft comments whose body starts with the robot emoji.

Call `add_review_comment` for each inline finding. During an automated review, call `finish_review` exactly once when the review is complete.

During `nitpick review chat`, additions and deletions stay local to the current batch until you call `finish_review`. Each successful call commits that batch to Nitpick and the pending GitHub review, then starts a new empty batch so you can continue reviewing. Call it again for each later batch, and do not exit with staged changes.
