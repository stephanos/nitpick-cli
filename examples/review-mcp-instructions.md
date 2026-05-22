Use the `nitpick-review` MCP server for review output.

Call `existing_review_comments` before reviewing so you can account for user comments and previous Nitpick comments.

Call `delete_draft_comment` for outdated Nitpick draft comments only when appropriate; it only accepts draft comments whose body starts with the robot emoji.

Call `add_review_comment` for each inline finding, then call `finish_review` exactly once when the review is complete.
