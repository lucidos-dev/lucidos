-- Rename payload fields to match ThreadEvent type definitions.
-- Old Event constructors used tool_name/content/arguments;
-- new ThreadEvent uses name/text/args.

-- ToolCalled: tool_name → name, arguments → args
UPDATE events SET payload = (payload - 'tool_name' - 'arguments')
  || jsonb_build_object('name', payload->'tool_name', 'args', payload->'arguments')
WHERE event_type = 'ToolCalled' AND payload ? 'tool_name';

-- ToolResult: tool_name → name, result stays as result
UPDATE events SET payload = (payload - 'tool_name')
  || jsonb_build_object('name', payload->'tool_name')
WHERE event_type = 'ToolResult' AND payload ? 'tool_name';

-- MessageReceived: content → text
UPDATE events SET payload = (payload - 'content')
  || jsonb_build_object('text', payload->'content')
WHERE event_type = 'MessageReceived' AND payload ? 'content';

-- TextStreamed: content → text
UPDATE events SET payload = (payload - 'content')
  || jsonb_build_object('text', payload->'content')
WHERE event_type = 'TextStreamed' AND payload ? 'content';

-- ResponseGenerated: content → text (keep for display, though type has no fields)
UPDATE events SET payload = (payload - 'content')
  || jsonb_build_object('text', payload->'content')
WHERE event_type = 'ResponseGenerated' AND payload ? 'content';

-- ResponseCancelled: content → text
UPDATE events SET payload = (payload - 'content')
  || jsonb_build_object('text', payload->'content')
WHERE event_type = 'ResponseCancelled' AND payload ? 'content';

-- ResponseFailed: error stays as error (already matches)

-- ClaudeCodeTextStreamed: content → text
UPDATE events SET payload = (payload - 'content')
  || jsonb_build_object('text', payload->'content')
WHERE event_type = 'ClaudeCodeTextStreamed' AND payload ? 'content';

-- ClaudeCodeToolCalled: tool_name → name, arguments → args
UPDATE events SET payload = (payload - 'tool_name' - 'arguments')
  || jsonb_build_object('name', payload->'tool_name', 'args', payload->'arguments')
WHERE event_type = 'ClaudeCodeToolCalled' AND payload ? 'tool_name';

-- ClaudeCodeToolResult: tool_name → name
UPDATE events SET payload = (payload - 'tool_name')
  || jsonb_build_object('name', payload->'tool_name')
WHERE event_type = 'ClaudeCodeToolResult' AND payload ? 'tool_name';

-- ClaudeCodeUserMessageSent: content → text
UPDATE events SET payload = (payload - 'content')
  || jsonb_build_object('text', payload->'content')
WHERE event_type = 'ClaudeCodeUserMessageSent' AND payload ? 'content';

-- Thinking: context_tokens/context_messages/trimmed → text
UPDATE events SET payload = (payload - 'context_tokens' - 'context_messages' - 'trimmed')
  || jsonb_build_object('text', 'Context: ' || COALESCE((payload->>'context_tokens'), '0') || ' tokens, ' || COALESCE((payload->>'context_messages'), '0') || ' messages' || CASE WHEN (payload->>'trimmed')::boolean THEN ' (trimmed)' ELSE '' END)
WHERE event_type = 'Thinking' AND payload ? 'context_tokens';

-- ScheduledTaskStarted: task_id stays (already matches)
-- ThreadTitleGenerated: title stays (already matches)
-- ThreadPinned/ThreadUnpinned: no payload fields to rename
-- SessionStarted/SessionEnded: thread_id stays
-- ChangeProposed/Applied/Discarded/Reverted: fields already match
