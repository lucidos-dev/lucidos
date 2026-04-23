import { describe, it, expect, beforeEach } from 'vitest';
import { focusedThreadId, focusedDraftId } from '../../store/store';
import {
  PastedImage,
  pastedImagesForCurrentThread,
  getPastedImages,
  addPastedImage,
  removePastedImage,
  clearPastedImages,
  hydratePastedImages,
  pastedImagesStorageKey,
  resetPastedImagesForTests,
} from './pastedImages';

const COMPOSE_DRAFT_ID = 'test-compose-draft';

const THREAD_A = 'thread-a';
const THREAD_B = 'thread-b';

const img1: PastedImage = { base64: 'AAA', mimeType: 'image/png' };
const img2: PastedImage = { base64: 'BBB', mimeType: 'image/jpeg' };
const img3: PastedImage = { base64: 'CCC', mimeType: 'image/png' };

beforeEach(() => {
  localStorage.clear();
  resetPastedImagesForTests();
  focusedThreadId.value = null;
  // Pin the compose draft id so legacy "null = compose" assertions remain
  // deterministic across tests.
  focusedDraftId.value = COMPOSE_DRAFT_ID;
});

describe('thread-scoped image isolation', () => {
  it('images added to thread A are not visible from thread B', () => {
    addPastedImage(THREAD_A, img1);
    addPastedImage(THREAD_A, img2);

    expect(getPastedImages(THREAD_A)).toEqual([img1, img2]);
    expect(getPastedImages(THREAD_B)).toEqual([]);
  });

  it('images added to compose are not visible from any thread', () => {
    addPastedImage(null, img1);

    expect(getPastedImages(null)).toEqual([img1]);
    expect(getPastedImages(THREAD_A)).toEqual([]);
    expect(getPastedImages(THREAD_B)).toEqual([]);
  });

  it('clearing thread A does not affect thread B', () => {
    addPastedImage(THREAD_A, img1);
    addPastedImage(THREAD_B, img2);

    clearPastedImages(THREAD_A);

    expect(getPastedImages(THREAD_A)).toEqual([]);
    expect(getPastedImages(THREAD_B)).toEqual([img2]);
  });

  it('removing image from thread A does not affect thread B', () => {
    addPastedImage(THREAD_A, img1);
    addPastedImage(THREAD_A, img2);
    addPastedImage(THREAD_B, img3);

    removePastedImage(THREAD_A, 0);

    expect(getPastedImages(THREAD_A)).toEqual([img2]);
    expect(getPastedImages(THREAD_B)).toEqual([img3]);
  });
});

describe('pastedImagesForCurrentThread tracks focusedThreadId', () => {
  it('returns thread A images when A is focused, B images when B is focused', () => {
    addPastedImage(THREAD_A, img1);
    addPastedImage(THREAD_B, img2);

    focusedThreadId.value = THREAD_A;
    expect(pastedImagesForCurrentThread.value).toEqual([img1]);

    focusedThreadId.value = THREAD_B;
    expect(pastedImagesForCurrentThread.value).toEqual([img2]);
  });

  it('switching from A (with images) to B (no draft) shows empty', () => {
    addPastedImage(THREAD_A, img1);

    focusedThreadId.value = THREAD_A;
    expect(pastedImagesForCurrentThread.value).toEqual([img1]);

    focusedThreadId.value = THREAD_B;
    expect(pastedImagesForCurrentThread.value).toEqual([]);
  });

  it('switching back to thread A restores its images', () => {
    addPastedImage(THREAD_A, img1);

    focusedThreadId.value = THREAD_A;
    expect(pastedImagesForCurrentThread.value).toEqual([img1]);

    focusedThreadId.value = THREAD_B;
    expect(pastedImagesForCurrentThread.value).toEqual([]);

    focusedThreadId.value = THREAD_A;
    expect(pastedImagesForCurrentThread.value).toEqual([img1]);
  });

  it('compose images appear when focusedThreadId is null', () => {
    addPastedImage(null, img1);

    focusedThreadId.value = null;
    expect(pastedImagesForCurrentThread.value).toEqual([img1]);

    focusedThreadId.value = THREAD_A;
    expect(pastedImagesForCurrentThread.value).toEqual([]);
  });
});

describe('localStorage persistence is thread-scoped', () => {
  it('add writes to a thread-scoped localStorage key', () => {
    addPastedImage(THREAD_A, img1);

    const raw = localStorage.getItem(pastedImagesStorageKey(THREAD_A));
    expect(raw).not.toBeNull();
    expect(JSON.parse(raw!)).toEqual([img1]);

    expect(localStorage.getItem(pastedImagesStorageKey(THREAD_B))).toBeNull();
  });

  it('compose images persist under the compose key, not under any thread', () => {
    addPastedImage(null, img1);

    const composeKey = pastedImagesStorageKey(null);
    expect(composeKey).toBe(`cognos-draft-images:${COMPOSE_DRAFT_ID}`);

    const raw = localStorage.getItem(composeKey);
    expect(raw).not.toBeNull();
    expect(JSON.parse(raw!)).toEqual([img1]);

    expect(localStorage.getItem(pastedImagesStorageKey(THREAD_A))).toBeNull();
  });

  it('clear removes the localStorage entry for that thread only', () => {
    addPastedImage(THREAD_A, img1);
    addPastedImage(THREAD_B, img2);

    clearPastedImages(THREAD_A);

    expect(localStorage.getItem(pastedImagesStorageKey(THREAD_A))).toBeNull();
    expect(localStorage.getItem(pastedImagesStorageKey(THREAD_B))).not.toBeNull();
  });

  it('hydratePastedImages loads images from localStorage scoped to the thread', () => {
    localStorage.setItem(pastedImagesStorageKey(THREAD_A), JSON.stringify([img1, img2]));

    expect(hydratePastedImages(THREAD_A)).toEqual([img1, img2]);
    expect(getPastedImages(THREAD_A)).toEqual([img1, img2]);
    // Hydrating thread A must not pollute thread B
    expect(getPastedImages(THREAD_B)).toEqual([]);
  });

  it('hydratePastedImages returns empty when localStorage has no entry', () => {
    expect(hydratePastedImages(THREAD_A)).toEqual([]);
    expect(getPastedImages(THREAD_A)).toEqual([]);
  });

  it('hydratePastedImages does not overwrite in-memory images already present', () => {
    addPastedImage(THREAD_A, img1);
    // Stale localStorage entry that disagrees with in-memory state
    localStorage.setItem(pastedImagesStorageKey(THREAD_A), JSON.stringify([img2, img3]));

    // In-memory wins (it's the source of truth once hydrated)
    expect(hydratePastedImages(THREAD_A)).toEqual([img1]);
    expect(getPastedImages(THREAD_A)).toEqual([img1]);
  });
});

describe('cross-thread leak regression', () => {
  it('paste in A, switch to B, send in B — B does not see A images', () => {
    // 1. User in thread A pastes an image
    focusedThreadId.value = THREAD_A;
    addPastedImage(THREAD_A, img1);
    expect(pastedImagesForCurrentThread.value).toEqual([img1]);

    // 2. User switches to thread B
    focusedThreadId.value = THREAD_B;

    // 3. User submits in B — the images shown for sending must be empty
    expect(pastedImagesForCurrentThread.value).toEqual([]);

    // 4. After clearing B's draft on send, A's images are still preserved
    clearPastedImages(THREAD_B);
    expect(getPastedImages(THREAD_A)).toEqual([img1]);
  });

  it('paste in compose, send (creates thread X), then click thread A — A unaffected', () => {
    // 1. User in compose pastes image
    focusedThreadId.value = null;
    addPastedImage(null, img1);

    // 2. User submits — compose images cleared, new thread X created and focused
    const NEW_THREAD = 'new-thread-x';
    clearPastedImages(null);
    focusedThreadId.value = NEW_THREAD;
    expect(pastedImagesForCurrentThread.value).toEqual([]);

    // 3. Pre-existing thread A had its own images — still there
    addPastedImage(THREAD_A, img2);
    focusedThreadId.value = THREAD_A;
    expect(pastedImagesForCurrentThread.value).toEqual([img2]);
  });
});
