/**
 * Tests for the "/" prefix opening the CC command menu from the prompt input.
 *
 * The handleInput function in PromptInput detects "/" at the start of input
 * in CC threads and sets ccMenuOpenRequest signal. CCControlMenu consumes
 * the signal and opens the menu with the filter text.
 */
import { describe, it, expect } from 'vitest';

/**
 * Mirrors the handleInput "/" detection logic from PromptInput.
 * Returns the value that would be set on ccMenuOpenRequest.
 *
 * @param inputValue - current text in the prompt
 * @param channel - thread channel (undefined = no thread / compose view)
 * @param inputModeType - input mode toggle ('do' | 'claude_code'), only used when no thread
 */
function detectSlashPrefix(
  inputValue: string,
  channel: string | undefined,
  inputModeType: 'do' | 'claude_code' = 'do',
): string | null {
  const hasThread = channel !== undefined;
  const isCCMode = channel === 'claude_code' || (!hasThread && inputModeType === 'claude_code');
  if (isCCMode && inputValue.startsWith('/')) {
    return inputValue.slice(1);
  }
  return null;
}

describe('slash prefix detection in handleInput', () => {
  it('detects "/" alone and returns empty filter', () => {
    expect(detectSlashPrefix('/', 'claude_code')).toBe('');
  });

  it('detects "/help" and returns "help" as filter', () => {
    expect(detectSlashPrefix('/help', 'claude_code')).toBe('help');
  });

  it('does not trigger in non-CC threads', () => {
    expect(detectSlashPrefix('/', 'chat')).toBeNull();
  });

  it('does not trigger for non-slash input', () => {
    expect(detectSlashPrefix('hello', 'claude_code')).toBeNull();
    expect(detectSlashPrefix('', 'claude_code')).toBeNull();
  });

  it('detects "/" in compose view with Claude mode toggled', () => {
    expect(detectSlashPrefix('/', undefined, 'claude_code')).toBe('');
    expect(detectSlashPrefix('/commit', undefined, 'claude_code')).toBe('commit');
  });

  it('does not trigger in compose view with Manifest mode', () => {
    expect(detectSlashPrefix('/', undefined, 'do')).toBeNull();
  });
});
