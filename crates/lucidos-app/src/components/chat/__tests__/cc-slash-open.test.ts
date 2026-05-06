/**
 * Tests for the "/" prefix opening the CC command menu from the prompt input.
 *
 * The handleInput function in PromptInput detects "/" at the start of input
 * in CC threads and sets ccMenuOpenRequest signal. CCControlMenu consumes
 * the signal and opens the menu with the filter text.
 */
import { describe, it, expect, beforeEach } from 'vitest';
import { inputMode } from '../../../store/store';
import type { ComposeChannelMode, ThreadMeta } from '../../../store/thread-events';
import { effectiveSendMode } from '../promptToggleMode';
import { _resetComposeDraftsForTesting, setDraft } from '../../../store/composeDrafts';

/** Mirrors the handleInput "/" detection logic from PromptInput, fed by the
 *  same effectiveSendMode helper that the live code uses. */
function detectSlashPrefix(
  inputValue: string,
  thread: { meta: { id: string; state: 'composing' | 'active'; channel: ThreadMeta['channel'] } } | undefined,
): string | null {
  const isCCMode = effectiveSendMode(thread) === 'claude_code';
  if (isCCMode && inputValue.startsWith('/')) {
    return inputValue.slice(1);
  }
  return null;
}

let nextId = 0;
const cc = (composeMode: ComposeChannelMode, channel: ThreadMeta['channel']) => {
  const id = `cc-${++nextId}`;
  setDraft(id, { text: '', images: [], mode: composeMode });
  return { meta: { id, state: 'composing' as const, channel } };
};
const active = (channel: ThreadMeta['channel']) => ({
  meta: { id: `active-${++nextId}`, state: 'active' as const, channel },
});

describe('slash prefix detection in handleInput', () => {
  beforeEach(() => {
    inputMode.value = { type: 'do' };
    _resetComposeDraftsForTesting();
    nextId = 0;
  });

  it('detects "/" alone and returns empty filter', () => {
    expect(detectSlashPrefix('/', active('claude_code'))).toBe('');
  });

  it('detects "/help" and returns "help" as filter', () => {
    expect(detectSlashPrefix('/help', active('claude_code'))).toBe('help');
  });

  it('does not trigger in non-CC threads', () => {
    expect(detectSlashPrefix('/', active('chat'))).toBeNull();
  });

  it('does not trigger for non-slash input', () => {
    expect(detectSlashPrefix('hello', active('claude_code'))).toBeNull();
    expect(detectSlashPrefix('', active('claude_code'))).toBeNull();
  });

  it('detects "/" in compose view with Claude mode toggled', () => {
    inputMode.value = { type: 'claude_code' };
    expect(detectSlashPrefix('/', undefined)).toBe('');
    expect(detectSlashPrefix('/commit', undefined)).toBe('commit');
  });

  it('does not trigger in compose view with Lucidos mode', () => {
    inputMode.value = { type: 'do' };
    expect(detectSlashPrefix('/', undefined)).toBeNull();
  });

  it('triggers on a composing draft toggled to Claude even though channel is still chat (regression)', () => {
    // Draft was started in Lucidos (channel='chat'), then user clicked Claude
    // on the toggle. composeMode is 'claude_code'; channel won't update until
    // send. The slash menu must follow composeMode here, same as the send path.
    expect(detectSlashPrefix('/', cc('claude_code', 'chat'))).toBe('');
  });

  it('does not trigger on a composing draft toggled back to Lucidos even if started in Claude', () => {
    expect(detectSlashPrefix('/', cc('lucidos', 'claude_code'))).toBeNull();
  });
});
