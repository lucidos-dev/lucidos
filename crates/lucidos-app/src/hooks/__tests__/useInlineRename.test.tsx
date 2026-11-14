// @vitest-environment jsdom
/**
 * What a rename-in-place field rests on, pinned away from any surface. Two
 * surfaces share these now, and a third will copy whichever it finds.
 *
 * Both were inline in one component and untested: the commit-once guard and
 * the focus inside the opening tap, which is the only way iOS raises the
 * keyboard. The ADR 0118 re-seed is pinned per surface as well.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';
import { useInlineRename } from '../useInlineRename';

let host: HTMLElement;
let rename: ReturnType<typeof vi.fn>;
/** What the last render read, so a test can drive the hook like a surface. */
let hook: ReturnType<typeof useInlineRename>;

function Probe({ served }: { served: string }) {
  hook = useInlineRename(served, rename as (next: string) => Promise<unknown>);
  return (
    <input
      ref={hook.inputRef}
      value={hook.draft}
      onInput={(e) => hook.setDraft((e.currentTarget as HTMLInputElement).value)}
    />
  );
}

function field(): HTMLInputElement {
  const el = host.querySelector('input');
  if (!el) throw new Error('the probe field is not rendered');
  return el;
}

async function settle(): Promise<void> {
  await new Promise((resolve) => setTimeout(resolve, 0));
}

/** Preact defers an effect past paint, so a re-seed lands a frame later than
 *  the render that triggered it. Poll rather than guess the delay. */
async function waitFor(done: () => boolean, budgetMs = 1000): Promise<void> {
  for (let waited = 0; waited < budgetMs; waited += 10) {
    if (done()) return;
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
}

describe('useInlineRename', () => {
  beforeEach(() => {
    rename = vi.fn(async () => true);
    host = document.createElement('div');
    document.body.appendChild(host);
    render(<Probe served="Nightly CI" />, host);
  });

  afterEach(() => {
    render(null, host);
    document.body.innerHTML = '';
  });

  it('focuses and selects the field inside the call that opens it', () => {
    // Synchronously, before any render: iOS raises the keyboard only for a
    // focus() the tap itself makes.
    hook.open();

    expect(document.activeElement).toBe(field());
  });

  it('commits once, however many times Enter and blur arrive', async () => {
    hook.open();
    await settle();
    hook.setDraft('Nightly');
    await settle();

    await Promise.all([hook.commit(), hook.commit(), hook.commit()]);

    expect(rename).toHaveBeenCalledTimes(1);
    expect(rename).toHaveBeenCalledWith('Nightly');
  });

  it('calls nothing for an empty or unchanged draft, and closes', async () => {
    hook.open();
    await settle();
    hook.setDraft('   ');
    await settle();

    await hook.commit();
    await settle();

    expect(rename).not.toHaveBeenCalled();
    expect(hook.renaming).toBe(false);
  });

  // The guard latches before the await. With the close after it, a rejected
  // rename refused every later Enter and blur, and the user's text with it.
  it('closes the field even when the rename rejects', async () => {
    rename.mockRejectedValueOnce(new Error('engine said no'));
    hook.open();
    await settle();
    hook.setDraft('Nightly');
    await settle();

    await expect(hook.commit()).rejects.toThrow('engine said no');
    await waitFor(() => hook.draft === 'Nightly CI');

    expect(hook.renaming).toBe(false);
    expect(hook.draft).toBe('Nightly CI');
  });

  it('follows the served value while idle, and holds the draft while open', async () => {
    render(<Probe served="Nightly" />, host);
    await waitFor(() => hook.draft === 'Nightly');
    expect(hook.draft).toBe('Nightly');

    hook.open();
    await settle();
    hook.setDraft('My own name');
    await settle();
    render(<Probe served="Renamed elsewhere" />, host);
    await settle();

    expect(hook.draft).toBe('My own name');
  });
});
