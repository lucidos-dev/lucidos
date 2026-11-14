-- Migrate automated CC follow-ups from MessageReceived to ClaudeCodePromptSent.
-- These are conflict resolution and code review prompts sent by apply_now,
-- not user-typed messages. They show up as ugly user messages in the chat UI.
UPDATE events
SET event_type = 'ClaudeCodePromptSent',
    payload = jsonb_set(
        payload - 'images' - 'device_id' - 'image_description',
        '{type}', '"ClaudeCodePromptSent"'
    )
WHERE event_type = 'MessageReceived'
  AND (
    payload->>'text' LIKE 'A merge conflict occurred%'
    OR payload->>'text' LIKE 'Run /pre-worktree-merge-code-review%'
  );
