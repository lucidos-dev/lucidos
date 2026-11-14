/**
 * A confirm or a prompt turns visible only once its chunk can draw it, and a
 * caller awaiting it always gets an answer (ADR 0436). `settleConfirm` and
 * `settlePrompt` are the one way each closes.
 */
import { describe, it, expect, afterEach } from 'vitest';
import {
  confirmState, promptState, registerAskDialogChunk, settleConfirm, settlePrompt, showConfirm, showPrompt,
} from '../store';

/** A dialog chunk that lands when the test says. */
function coldChunk() {
  let finish: (loaded: boolean) => void = () => {};
  let loaded = false;
  const arrival = new Promise<boolean>((resolve) => { finish = resolve; });
  return {
    gate: { preload: () => arrival.then((ok) => { loaded = ok; return ok; }), isLoaded: () => loaded },
    land: (ok: boolean) => finish(ok),
  };
}

const settle = () => new Promise((r) => setTimeout(r, 0));

afterEach(() => registerAskDialogChunk(null));

describe('settling the ask dialogs', () => {
  it('answers a waiting confirm and closes it', async () => {
    const answer = showConfirm('Delete it?', 'Delete', { variant: 'danger' });
    expect(confirmState.value.visible).toBe(true);
    settleConfirm(false);
    await expect(answer).resolves.toBe(false);
    expect(confirmState.value.visible).toBe(false);
  });

  it('answers a waiting prompt and closes it', async () => {
    const answer = showPrompt('Name it');
    expect(promptState.value.visible).toBe(true);
    settlePrompt(null);
    await expect(answer).resolves.toBeNull();
    expect(promptState.value.visible).toBe(false);
  });
});

describe('a dialog whose chunk is still on its way', () => {
  it('turns visible only once the chunk lands', async () => {
    const chunk = coldChunk();
    registerAskDialogChunk(chunk.gate);
    const answer = showConfirm('Delete it?', 'Delete', { variant: 'danger' });
    expect(confirmState.value.visible, 'no visible state with nothing to draw it').toBe(false);
    chunk.land(true);
    await settle();
    expect(confirmState.value.visible).toBe(true);
    settleConfirm(true);
    await expect(answer).resolves.toBe(true);
  });

  it('declines a waiting call that a later one replaced', async () => {
    const chunk = coldChunk();
    registerAskDialogChunk(chunk.gate);
    const first = showPrompt('First');
    const second = showPrompt('Second');
    chunk.land(true);
    await expect(first).resolves.toBeNull();
    await settle();
    expect(promptState.value.message).toBe('Second');
    settlePrompt('named');
    await expect(second).resolves.toBe('named');
  });

  it('declines when the chunk cannot load, and never turns visible', async () => {
    const chunk = coldChunk();
    registerAskDialogChunk(chunk.gate);
    const answer = showConfirm('Delete it?', 'Delete', { variant: 'danger' });
    chunk.land(false);
    await expect(answer).resolves.toBe(false);
    expect(confirmState.value.visible).toBe(false);
  });
});
