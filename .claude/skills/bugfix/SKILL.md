---
name: bugfix
description: Use when fixing any bug — enforces test-first approach with integration tests before code changes
---

# Fix Bugs — Test-First, Integration-First

## The Rule

**Never fix a bug by changing code first.** Always:

1. **Reproduce** — understand exactly what's wrong
2. **Write a failing test** — if you can't write a test that fails, you don't understand the bug
3. **Fix the code** — make the test pass
4. **Verify** — run full test suite, check for regressions

## Integration Tests Over Unit Tests

For bugs that involve data flowing between components (SSE → store → rendering), write **integration tests** that simulate the full flow, not just unit tests for individual functions.

### What an integration test looks like

```typescript
it('CC session shows Idle status, not Done', () => {
  // 1. Set up: create thread, simulate SSE events
  const map = new Map<string, ThreadState>();
  const threadId = 'test-cc';
  map.set(threadId, makeThreadState());

  // 2. Simulate the full event sequence as it arrives from SSE
  handleEvent(map, threadId, -1, { type: 'MessageReceived', text: 'fix the bug' });
  handleEvent(map, threadId, -2, { type: 'SessionStarted', session_id: 's1' });
  handleEvent(map, threadId, -3, { type: 'ClaudeCodeToolCalled', name: 'Edit', args: {} });
  handleEvent(map, threadId, -4, { type: 'ClaudeCodeToolResult', name: 'Edit', result: 'ok' });
  handleEvent(map, threadId, -5, { type: 'ResponseGenerated' });
  handleEvent(map, threadId, -6, { type: 'ClaudeCodeIdled' });

  // 3. Assert the full pipeline produces correct output
  const exchanges = groupIntoExchanges(map.get(threadId)!.events);
  const chats = exchangesToChats(exchanges, '');

  // 4. Verify the user-visible result
  expect(chats[0].status).toBe('cc-idle');  // NOT 'done'
});
```

### What to test

For each bug, test the **complete data flow**, not just the function that's broken:

| Layer | What to verify |
|-------|---------------|
| SSE → handleEvent | Events inserted correctly, pendingUserMessage converted |
| handleEvent → groupIntoExchanges | Exchanges have correct boundaries and steps |
| groupIntoExchanges → adapter | ChatExchange has correct status, response, events |
| adapter → component | The right data reaches the rendering component |

### When to write integration tests

- **Before any refactor** that changes data flow between components
- **For every bug** that involves incorrect rendering, wrong status, missing data
- **For every flow** (new chat, follow-up, CC session, scheduled task, CC follow-up, recovery)

## The Checklist

Before claiming a bug is fixed:

- [ ] Failing test exists that reproduces the bug
- [ ] Test covers the full flow (not just the broken function)
- [ ] Fix makes the test pass
- [ ] All existing tests still pass
- [ ] `cargo test -p cognos-engine` passes
- [ ] `cd crates/cognos-app && npm test` passes
- [ ] `npx tsc --noEmit` — zero errors
- [ ] No new warnings

## Red Flags

| Thought | Reality |
|---------|---------|
| "The fix is obvious, I'll just change it" | Write the test first. The fix might break something else. |
| "Unit tests pass, it's fine" | Unit tests don't test wiring. Write an integration test. |
| "I'll add tests later" | No. Tests first, always. |
| "This is just a one-line fix" | One-line fixes break things too. Test it. |
| "The old code didn't have tests" | That's why it had bugs. Write them now. |
