/**
 * Tests for the "/" that opens the control command menu from the prompt input.
 *
 * `handleInput` in PromptInput asks `opensSlashMenu` whether this keystroke
 * opens the menu, and clears the box when it does. CodingAgentControlMenu then
 * consumes `codingAgentMenuOpenRequest` and opens.
 */
import { describe, it, expect, beforeEach } from 'vitest';
import { inputMode } from '../../../store/store';
import type { CodingAgent } from '../../../api/types';
import type { ComposeChannelMode, ThreadMeta } from '../../../store/thread-events';
import { effectiveCodingAgentBackend } from '../promptToggleMode';
import { opensSlashMenu } from '../PromptInput';
import { _resetComposeDraftsForTesting, setDraft } from '../../../store/composeDrafts';

/** The `handleInput` decision, fed by the same helpers the live code uses. */
function detectSlash(
  inputValue: string,
  thread: { meta: { id: string; state: 'composing' | 'active'; channel: ThreadMeta['channel']; codingAgent?: CodingAgent } } | undefined,
  selectedAgent: CodingAgent = 'claude-code',
): boolean {
  return opensSlashMenu(inputValue, effectiveCodingAgentBackend(thread, selectedAgent) === 'claude-code');
}

let nextId = 0;
const cc = (composeMode: ComposeChannelMode, channel: ThreadMeta['channel']) => {
  const id = `cc-${++nextId}`;
  setDraft(id, { text: '', image_hashes: [], mode: composeMode });
  return { meta: { id, state: 'composing' as const, channel } };
};
const active = (channel: ThreadMeta['channel']) => ({
  meta: { id: `active-${++nextId}`, state: 'active' as const, channel },
});
const activeCodingAgent = (codingAgent: CodingAgent) => ({
  meta: { id: `active-${++nextId}`, state: 'active' as const, channel: 'claude_code' as const, codingAgent },
});

describe('slash detection in handleInput', () => {
  beforeEach(() => {
    inputMode.value = { type: 'do' };
    _resetComposeDraftsForTesting();
    nextId = 0;
  });

  it('opens on a lone "/"', () => {
    expect(detectSlash('/', active('claude_code'))).toBe(true);
  });

  // Regression: the menu opens by clearing the box, and it used to fire on any
  // value STARTING with "/". The user's text then moved into the menu filter,
  // and closing the menu lost it for good.
  it('keeps a pasted path as text', () => {
    expect(detectSlash('/Users/me/project/src/foo.ts:12 throws', active('claude_code'))).toBe(false);
  });

  it('keeps a draft the user typed a slash in front of', () => {
    expect(detectSlash('/fix the flaky test', active('claude_code'))).toBe(false);
  });

  it('does not trigger in non-CC threads', () => {
    expect(detectSlash('/', active('chat'))).toBe(false);
  });

  it('does not trigger for non-slash input', () => {
    expect(detectSlash('hello', active('claude_code'))).toBe(false);
    expect(detectSlash('', active('claude_code'))).toBe(false);
  });

  it('detects "/" in compose view with Claude mode toggled', () => {
    inputMode.value = { type: 'coding_agent' };
    expect(detectSlash('/', undefined)).toBe(true);
  });

  it('does not trigger in compose view when Codex is selected', () => {
    inputMode.value = { type: 'coding_agent' };
    expect(detectSlash('/', undefined, 'codex')).toBe(false);
  });

  it('does not trigger in compose view with Lucidos mode', () => {
    inputMode.value = { type: 'do' };
    expect(detectSlash('/', undefined)).toBe(false);
  });

  it('triggers on a composing draft toggled to Claude even though channel is still chat (regression)', () => {
    // Draft was started in Lucidos (channel='chat'), then user clicked Claude
    // on the toggle. composeMode is 'claude_code'; channel won't update until
    // send. The slash menu must follow composeMode here, same as the send path.
    expect(detectSlash('/', cc('claude_code', 'chat'))).toBe(true);
  });

  it('does not trigger on a composing Codex draft even though channel is coding-agent', () => {
    expect(detectSlash('/', cc('claude_code', 'claude_code'), 'codex')).toBe(false);
  });

  it('does not trigger on an active Codex thread', () => {
    expect(detectSlash('/', activeCodingAgent('codex'))).toBe(false);
  });

  it('does not trigger on a composing draft toggled back to Lucidos even if started in Claude', () => {
    expect(detectSlash('/', cc('lucidos', 'claude_code'))).toBe(false);
  });
});
