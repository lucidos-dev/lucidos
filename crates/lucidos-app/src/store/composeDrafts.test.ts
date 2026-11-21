/** `draftPresentThreadIds` is a coarse signal exposing only WHICH threads
 *  carry a non-empty draft — never the draft contents. The drawer's draft
 *  indicator subscribes to it instead of `composeDrafts`, so per-keystroke
 *  text mutations don't fan a re-render across every visible ThreadRow.
 *
 *  These tests pin the contract: the signal reference must stay stable across
 *  same-presence keystrokes (the perf invariant) and must flip on the
 *  empty↔non-empty boundary (the correctness invariant). Without the
 *  same-presence stability guard, ThreadRows re-render per character — the
 *  exact regression that prompted this signal.
 */
import { afterEach, describe, expect, it } from 'vitest';
import {
  _resetComposeDraftsForTesting,
  applyDraftBatch,
  clearDraft,
  draftPresentThreadIds,
  patchDraft,
  setDraft,
} from './composeDrafts';

afterEach(() => {
  _resetComposeDraftsForTesting();
});

describe('draftPresentThreadIds membership', () => {
  it('starts empty', () => {
    expect(draftPresentThreadIds.value.size).toBe(0);
  });

  it('adds a thread on the empty→non-empty transition', () => {
    patchDraft('t-1', { text: 'a' });
    expect(draftPresentThreadIds.value.has('t-1')).toBe(true);
  });

  it('removes a thread when the draft becomes empty again', () => {
    patchDraft('t-1', { text: 'a' });
    patchDraft('t-1', { text: '' });
    expect(draftPresentThreadIds.value.has('t-1')).toBe(false);
  });

  it('treats whitespace-only text as empty (parity with draftIsEmpty)', () => {
    patchDraft('t-1', { text: '   \n\t' });
    expect(draftPresentThreadIds.value.has('t-1')).toBe(false);
  });

  it('counts an image-only draft as present', () => {
    patchDraft('t-1', { image_hashes: ['hash-a'] });
    expect(draftPresentThreadIds.value.has('t-1')).toBe(true);
  });

  it('drops the entry when clearDraft removes the draft', () => {
    patchDraft('t-1', { text: 'hello' });
    clearDraft('t-1');
    expect(draftPresentThreadIds.value.has('t-1')).toBe(false);
  });

  it('setDraft flips presence in both directions', () => {
    setDraft('t-1', { text: 'hi', image_hashes: [], mode: null });
    expect(draftPresentThreadIds.value.has('t-1')).toBe(true);
    setDraft('t-1', { text: '', image_hashes: [], mode: null });
    expect(draftPresentThreadIds.value.has('t-1')).toBe(false);
  });

  it('applyDraftBatch reflects every transition once', () => {
    setDraft('t-1', { text: 'present', image_hashes: [], mode: null });
    setDraft('t-2', { text: '', image_hashes: [], mode: null });
    applyDraftBatch(new Map([
      ['t-1', null],
      ['t-2', { text: 'now present', image_hashes: [], mode: null }],
      ['t-3', { text: 'fresh', image_hashes: [], mode: null }],
    ]));
    expect(draftPresentThreadIds.value.has('t-1')).toBe(false);
    expect(draftPresentThreadIds.value.has('t-2')).toBe(true);
    expect(draftPresentThreadIds.value.has('t-3')).toBe(true);
  });
});

/** ThreadRow subscribes to draftPresentThreadIds via threadHasUnsentDraft —
 *  if its reference flips on a no-op keystroke, every visible row re-renders
 *  per character. That regression is the exact symptom of this bug. */
describe('draftPresentThreadIds reference stability (perf isolation)', () => {
  it('keystrokes inside a non-empty draft do NOT change the signal reference', () => {
    patchDraft('t-1', { text: 'a' });
    const before = draftPresentThreadIds.value;
    patchDraft('t-1', { text: 'ab' });
    patchDraft('t-1', { text: 'abc' });
    patchDraft('t-1', { text: 'abcd' });
    expect(draftPresentThreadIds.value).toBe(before);
  });

  it('keystrokes inside an empty draft (no presence to flip) do NOT change the signal reference', () => {
    const before = draftPresentThreadIds.value;
    patchDraft('t-1', { text: '' });
    patchDraft('t-1', { text: '   ' });
    expect(draftPresentThreadIds.value).toBe(before);
  });

  it('image_hashes mutations on an already-present draft do NOT change the signal reference', () => {
    patchDraft('t-1', { text: 'present' });
    const before = draftPresentThreadIds.value;
    patchDraft('t-1', { image_hashes: ['h1'] });
    patchDraft('t-1', { image_hashes: ['h1', 'h2'] });
    expect(draftPresentThreadIds.value).toBe(before);
  });

  it('the empty→non-empty transition DOES change the signal reference (correctness, not stability)', () => {
    const before = draftPresentThreadIds.value;
    patchDraft('t-1', { text: 'first character' });
    expect(draftPresentThreadIds.value).not.toBe(before);
  });
});
