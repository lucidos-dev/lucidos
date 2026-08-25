// @vitest-environment jsdom
/**
 * A re-read that was already in flight when the user started typing must drop
 * its reply.
 *
 * The obvious guard is the wrong one. Both surfaces gate whether a re-read
 * STARTS, on `edited` and on `dirty`, and both read that at render time. The
 * reply lands between renders, so a read begun one tick before the first
 * keystroke still resolves and overwrites the draft. Silent data loss, and no
 * test saw it: the ordinary path never has a fetch open while the user types.
 *
 * Each case therefore drives the promise by hand, so the keystroke lands in
 * the window that only exists mid-flight.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';

/** A promise whose settle time the test owns. */
function deferred<T>(): { promise: Promise<T>; resolve: (v: T) => void } {
  let resolve!: (v: T) => void;
  const promise = new Promise<T>((r) => { resolve = r; });
  return { promise, resolve };
}

const readAppSource = vi.fn();
vi.mock('../../api/client', async () => {
  const actual = await vi.importActual<Record<string, unknown>>('../../api/client');
  return { ...actual, readAppSourceApi: (...args: unknown[]) => readAppSource(...args) };
});

import { AppUiEditModal } from '../apps/AppUiEditModal';
import { AllowlistEditor } from '../settings/AllowlistEditor';
import { appsList, panelOverlay, appSourceEpoch, permissionGrantsVersion } from '../../store/store';
import type { App } from '../../store/types';

const APP_ID = 'habit-tracker';

function app(): App {
  return { id: APP_ID, name: 'Habit Tracker', description: 'Tracks habits' };
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

describe('the app source editor drops a re-read the user typed under', () => {
  let host: HTMLElement;

  function sourceBox(): HTMLTextAreaElement {
    const el = host.querySelector<HTMLTextAreaElement>('textarea.code-textarea');
    if (!el) throw new Error('the source editor is not rendered');
    return el;
  }

  beforeEach(() => {
    document.body.innerHTML = '';
    host = document.createElement('div');
    document.body.appendChild(host);
    readAppSource.mockReset();
    appsList.value = { status: 'loaded', data: [app()] };
    panelOverlay.value = { type: 'form', form: { type: 'app-edit', appId: APP_ID } };
  });

  afterEach(() => {
    render(null, host);
    document.body.innerHTML = '';
    panelOverlay.value = null;
    appsList.value = { status: 'not-loaded' };
    appSourceEpoch.value = 0;
  });

  it('keeps the edit when the epoch read resolves after the keystroke', async () => {
    readAppSource.mockResolvedValueOnce({ files: [{ name: 'index.html', content: 'ORIGINAL' }] });
    render(<AppUiEditModal />, host);
    await waitFor(() => host.querySelector('textarea.code-textarea') !== null);

    // A coding-agent apply lands: the epoch moves and a re-read starts, but
    // the engine has not answered yet.
    const inFlight = deferred<{ files: { name: string; content: string }[] }>();
    readAppSource.mockReturnValueOnce(inFlight.promise);
    appSourceEpoch.value++;
    await waitFor(() => readAppSource.mock.calls.length === 2);

    // The user types INTO that window.
    typeInto(sourceBox(), 'MY UNSAVED EDIT');
    await waitFor(() => sourceBox().value === 'MY UNSAVED EDIT');

    // Only now does the read answer, with the bytes that predate the edit.
    inFlight.resolve({ files: [{ name: 'index.html', content: 'FROM THE AGENT' }] });
    await settle();

    expect(sourceBox().value).toBe('MY UNSAVED EDIT');
  });

  it('still swaps in the new bytes when nobody typed', async () => {
    readAppSource.mockResolvedValueOnce({ files: [{ name: 'index.html', content: 'ORIGINAL' }] });
    render(<AppUiEditModal />, host);
    await waitFor(() => host.querySelector('textarea.code-textarea') !== null);

    readAppSource.mockResolvedValueOnce({
      files: [{ name: 'index.html', content: 'FROM THE AGENT' }],
    });
    appSourceEpoch.value++;
    await waitFor(() => sourceBox().value === 'FROM THE AGENT');

    expect(sourceBox().value).toBe('FROM THE AGENT');
  });
});

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
