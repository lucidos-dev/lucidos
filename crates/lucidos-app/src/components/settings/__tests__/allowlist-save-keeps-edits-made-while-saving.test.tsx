// @vitest-environment jsdom
/**
 * A save puts back what it saved, and only that. Rows typed or added while
 * the save was in flight are a newer draft, so they must stay on screen and
 * keep the editor dirty.
 *
 * Regression: the save's success branch replaced the rows with its own
 * click-time snapshot. A pattern added during the round trip vanished, the
 * editor read clean, and nothing said anything was lost.
 */
import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

import { AllowlistEditor } from '../AllowlistEditor';

/** Lets pending promises land, then flushes Preact's renders and effects. The
 *  mount load starts in an effect, which Preact defers past a paint. */
function settled(): Promise<void> {
  return act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 0));
  });
}

function deferred(): { promise: Promise<void>; resolve: () => void } {
  let resolve!: () => void;
  const promise = new Promise<void>((r) => { resolve = r; });
  return { promise, resolve };
}

describe('AllowlistEditor save', () => {
  let host: HTMLElement;

  function buttonSaying(text: string): HTMLButtonElement {
    const el = [...host.querySelectorAll<HTMLButtonElement>('button')]
      .find((b) => b.textContent?.trim() === text);
    if (!el) throw new Error(`no "${text}" button`);
    return el;
  }
  function inputs(): HTMLInputElement[] {
    return [...host.querySelectorAll<HTMLInputElement>('.allowlist-row-input')];
  }
  function type(input: HTMLInputElement, value: string): void {
    input.value = value;
    input.dispatchEvent(new Event('input', { bubbles: true }));
  }

  beforeEach(() => {
    host = document.createElement('div');
    document.body.appendChild(host);
  });

  afterEach(() => {
    render(null, host);
    host.remove();
  });

  it('keeps a pattern added while the save is in flight', async () => {
    const pendingSave = deferred();
    const saved: string[] = [];
    await act(() => render(
      <AllowlistEditor
        title="Tool permissions"
        anchor="permissions:tools"
        description="what it is for"
        placeholder="Bash(git:*)"
        noun="tool permissions"
        load={() => Promise.resolve('# header\nBash(git:*)\n')}
        save={(contents) => { saved.push(contents); return pendingSave.promise; }}
      />,
      host,
    ));
    await settled();

    buttonSaying('Add pattern').click();
    await settled();
    type(inputs()[1], 'Python');
    await settled();
    buttonSaying('Save').click();
    await settled();

    // Typed while the save is still on the wire.
    buttonSaying('Add pattern').click();
    await settled();
    type(inputs()[2], 'Bash(ls:*)');
    await settled();

    pendingSave.resolve();
    await settled();

    expect(saved).toEqual(['# header\nBash(git:*)\nPython\n']);
    expect(inputs().map((i) => i.value)).toEqual(['Bash(git:*)', 'Python', 'Bash(ls:*)']);
    // The newer draft is unsaved, so Save offers to write it.
    expect(buttonSaying('Save').disabled).toBe(false);
  });

  it('collapses the saved rows to their persisted form when nothing moved', async () => {
    const pendingSave = deferred();
    await act(() => render(
      <AllowlistEditor
        title="Tool permissions"
        anchor="permissions:tools"
        description="what it is for"
        placeholder="Bash(git:*)"
        noun="tool permissions"
        load={() => Promise.resolve('Bash(git:*)\n')}
        save={() => pendingSave.promise}
      />,
      host,
    ));
    await settled();

    buttonSaying('Add pattern').click();
    await settled();
    type(inputs()[1], '  Python  ');
    buttonSaying('Add pattern').click();
    await settled();
    buttonSaying('Save').click();
    await settled();
    pendingSave.resolve();
    await settled();

    expect(inputs().map((i) => i.value)).toEqual(['Bash(git:*)', 'Python']);
    expect(buttonSaying('Save').disabled).toBe(true);
  });
});
