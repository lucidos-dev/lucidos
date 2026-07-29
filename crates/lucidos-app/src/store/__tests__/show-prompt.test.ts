import { describe, it, expect, beforeEach } from 'vitest';
import { showPrompt, promptState } from '../store';

describe('showPrompt', () => {
  beforeEach(() => {
    // Reset any leftover dialog from a prior test.
    promptState.value = { visible: false, message: '' };
  });

  it('opens the dialog with the supplied options', () => {
    void showPrompt('New name?', { title: 'Rename', defaultValue: 'Untitled', okLabel: 'Save', multiline: true });
    const s = promptState.value;
    expect(s.visible).toBe(true);
    expect(s.message).toBe('New name?');
    expect(s.title).toBe('Rename');
    expect(s.defaultValue).toBe('Untitled');
    expect(s.okLabel).toBe('Save');
    expect(s.multiline).toBe(true);
  });

  it('resolves the entered string when its resolver is called (OK)', async () => {
    const p = showPrompt('Name?');
    promptState.value.resolve?.('Alice');
    await expect(p).resolves.toBe('Alice');
  });

  it('resolves null when cancelled', async () => {
    const p = showPrompt('Name?');
    promptState.value.resolve?.(null);
    await expect(p).resolves.toBeNull();
  });

  it('a second prompt resolves the prior one as null and replaces it (never queues)', async () => {
    const first = showPrompt('First?');
    const second = showPrompt('Second?');
    // The prior prompt is auto-resolved null by the replace.
    await expect(first).resolves.toBeNull();
    expect(promptState.value.message).toBe('Second?');
    promptState.value.resolve?.('done');
    await expect(second).resolves.toBe('done');
  });
});
