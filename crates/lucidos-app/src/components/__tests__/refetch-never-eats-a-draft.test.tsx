// @vitest-environment jsdom
/**
 * A re-read that was already in flight when the user started typing must drop
 * its reply.
 *
 * The obvious guard is the wrong one. The editor gates whether a re-read
 * STARTS on `dirty`, and reads it at render time. The reply lands between
 * renders, so a read begun one tick before the first keystroke still resolves
 * and overwrites the draft. Silent data loss, and no
 * test saw it: the ordinary path never has a fetch open while the user types.
 *
 * Each case therefore drives the promise by hand, so the keystroke lands in
 * the window that only exists mid-flight.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';
import { AllowlistEditor } from '../settings/AllowlistEditor';
import { permissionGrantsVersion } from '../../store/store';

/** A promise whose settle time the test owns. */
function deferred<T>(): { promise: Promise<T>; resolve: (v: T) => void } {
  let resolve!: (v: T) => void;
  const promise = new Promise<T>((r) => { resolve = r; });
  return { promise, resolve };
}

async function waitFor(done: () => boolean, budgetMs = 1000): Promise<void> {
  for (let waited = 0; waited < budgetMs; waited += 10) {
    if (done()) return;
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
}

async function settle(): Promise<void> {
  await new Promise((resolve) => setTimeout(resolve, 120));
}

function typeInto(el: HTMLInputElement | HTMLTextAreaElement, value: string): void {
  el.value = value;
  el.dispatchEvent(new Event('input', { bubbles: true }));
}

describe('the allowlist editor drops a re-read the user typed under', () => {
  let host: HTMLElement;

  function rows(): HTMLInputElement[] {
    return [...host.querySelectorAll<HTMLInputElement>('.allowlist-row-input')];
  }

  function editorProps(load: () => Promise<string>) {
    return {
      title: 'Tool permissions',
      anchor: 'permissions-tools',
      description: 'What the coding agent may run unattended.',
      placeholder: 'e.g. Bash(git status)',
      noun: 'tool permissions',
      load,
      save: vi.fn(async () => {}),
    };
  }

  beforeEach(() => {
    document.body.innerHTML = '';
    host = document.createElement('div');
    document.body.appendChild(host);
  });

  afterEach(() => {
    render(null, host);
    document.body.innerHTML = '';
    permissionGrantsVersion.value = 0;
  });

  it('keeps the typed pattern when the reply predates it', async () => {
    let next: Promise<string> = Promise.resolve('# header\nBash(git status)\n');
    const load = vi.fn(() => next);
    render(<AllowlistEditor {...editorProps(load)} />, host);
    await waitFor(() => rows().length === 1);

    // A grant lands from the agent, so the editor re-reads. Still in flight.
    const inFlight = deferred<string>();
    next = inFlight.promise;
    permissionGrantsVersion.value++;
    await waitFor(() => load.mock.calls.length === 2);

    typeInto(rows()[0], 'Bash(git log)');
    await waitFor(() => rows()[0].value === 'Bash(git log)');

    inFlight.resolve('# header\nBash(git status)\nBash(gh pr list)\n');
    await settle();

    expect(rows()[0].value).toBe('Bash(git log)');
  });

  it('still takes the new file when nobody typed', async () => {
    let next: Promise<string> = Promise.resolve('# header\nBash(git status)\n');
    const load = vi.fn(() => next);
    render(<AllowlistEditor {...editorProps(load)} />, host);
    await waitFor(() => rows().length === 1);

    next = Promise.resolve('# header\nBash(git status)\nBash(gh pr list)\n');
    permissionGrantsVersion.value++;
    await waitFor(() => rows().length === 2);

    expect(rows().map((r) => r.value)).toEqual(['Bash(git status)', 'Bash(gh pr list)']);
  });
});
