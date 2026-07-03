/** Compose destination — pure mapping/encoding helpers plus the
 *  `applyDestination` write path (inputMode + selectedScope + composing
 *  draft mode). The wire shape is pinned separately by
 *  `actions/compose-toggle-channel.test.ts`; this suite covers the picker's
 *  state plumbing. */

import { describe, it, expect, beforeEach, vi } from 'vitest';

vi.hoisted(() => {
  const storage = new Map<string, string>();
  (globalThis as any).localStorage = {
    getItem: (k: string) => storage.get(k) ?? null,
    setItem: (k: string, v: string) => storage.set(k, v),
    removeItem: (k: string) => storage.delete(k),
    clear: () => storage.clear(),
    get length() { return storage.size; },
    key: (_i: number) => null,
  };
  if (typeof globalThis.document === 'undefined') {
    (globalThis as any).document = {};
  }
  if (!(globalThis.document as any).querySelector) {
    (globalThis.document as any).querySelector = () => null;
  }
  if (!(globalThis.document as any).querySelectorAll) {
    (globalThis.document as any).querySelectorAll = () => [];
  }
  if (typeof globalThis.requestAnimationFrame === 'undefined') {
    (globalThis as any).requestAnimationFrame = (cb: any) => { cb(); return 0; };
  }
  if (typeof globalThis.crypto === 'undefined' || !(globalThis.crypto as any).randomUUID) {
    (globalThis as any).crypto = {
      randomUUID: () => 'test-uuid-' + Math.random().toString(36).slice(2),
    };
  }
});

vi.mock('../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../api/client')>()),
  API_BASE: '',
  putComposeOnThread: vi.fn().mockResolvedValue(undefined),
  ensureThreadStarted: vi.fn().mockResolvedValue(undefined),
  deleteThread: vi.fn().mockResolvedValue(undefined),
}));

vi.mock('./actions/thread-navigation', () => ({
  pushThreadNavState: vi.fn(),
  removeThreadNavEntries: vi.fn(),
}));

vi.mock('./actions/devices', () => ({
  getDeviceId: () => 'device-test',
}));

vi.mock('../utils/platform', () => ({
  isTauri: () => false,
  isIOS: () => false,
}));

import { inputMode, selectedScope, threadMap, type Scope } from './store';
import {
  destinationFromState,
  destinationToOptionValue,
  parseOptionValue,
  destinationCaption,
  composeDraftContextName,
  LUCIDOS_SOURCE_REPO_NAME,
  REGISTER_REPO_OPTION_VALUE,
  type ComposeDestination,
} from './composeDestination';
import { applyDestination } from './actions/compose';
import { getDraft, setDraft, _resetComposeDraftsForTesting } from './composeDrafts';
import { getComposeSelectionOverride, pendingComposeSelection, resolveScope, _resetComposeSelectionsForTesting } from './composeSelections';
import { makeOptimisticThreadState } from './thread-events';

function putThread(id: string, state: 'composing' | 'active'): void {
  const next = new Map(threadMap.value);
  next.set(id, makeOptimisticThreadState({
    id,
    title: '',
    channel: 'chat',
    initiator: 'user',
    eventsLoaded: true,
    state,
    status: 'idle',
  }));
  threadMap.value = next;
}

beforeEach(() => {
  threadMap.value = new Map();
  inputMode.value = { type: 'do' };
  selectedScope.value = { kind: 'lucidos' };
  _resetComposeDraftsForTesting();
  _resetComposeSelectionsForTesting();
});

describe('option value encoding round-trips', () => {
  const cases: Array<[ComposeDestination, string]> = [
    [{ kind: 'lucidos-agent' }, 'agent'],
    [{ kind: 'coding', scope: { kind: 'lucidos' } }, 'code:lucidos'],
    [{ kind: 'coding', scope: { kind: 'external', repoId: 'uuid-1' } }, 'repo:uuid-1'],
    [{ kind: 'coding', scope: { kind: 'app', appId: 'habit-tracker' } }, 'app:habit-tracker'],
  ];

  it.each(cases)('%j ↔ %s', (dest, encoded) => {
    expect(destinationToOptionValue(dest)).toBe(encoded);
    expect(parseOptionValue(encoded)).toEqual(dest);
  });

  it('unknown values fall back to the Lucidos Agent', () => {
    expect(parseOptionValue('__hdr-coding')).toEqual({ kind: 'lucidos-agent' });
    expect(parseOptionValue('')).toEqual({ kind: 'lucidos-agent' });
    // The register-repo sentinel is NOT a destination — callers intercept it
    // before parsing; parseOptionValue itself treats it as unknown.
    expect(parseOptionValue(REGISTER_REPO_OPTION_VALUE)).toEqual({ kind: 'lucidos-agent' });
  });
});

describe('destinationFromState', () => {
  it('lucidos mode is the Lucidos Agent regardless of scope', () => {
    const scope: Scope = { kind: 'external', repoId: 'r1' };
    expect(destinationFromState('lucidos', scope)).toEqual({ kind: 'lucidos-agent' });
  });

  it('claude_code mode carries the scope as the coding target', () => {
    const scope: Scope = { kind: 'app', appId: 'a1' };
    expect(destinationFromState('claude_code', scope)).toEqual({ kind: 'coding', scope });
  });
});

describe('applyDestination', () => {
  it('coding target on a composing draft sets inputMode + the draft mode + the per-draft scope AND the last-used seed, without leaking to other drafts', () => {
    const id = 'dest-coding';
    putThread(id, 'composing');
    setDraft(id, { text: 'fix it', image_hashes: [], mode: 'lucidos' });

    applyDestination(id, { kind: 'coding', scope: { kind: 'app', appId: 'a1' } });

    expect(inputMode.value).toEqual({ type: 'coding_agent' });
    // The target is stored on THIS draft's override…
    expect(getComposeSelectionOverride(id).scope).toEqual({ kind: 'app', appId: 'a1' });
    // …and mirrored to the localStorage last-used seed so the NEXT new draft
    // starts from it. This is leak-safe: an EXISTING draft resolves its OWN scope
    // (or the fixed {lucidos} default), never the shared selectedScope — so this
    // pick does not move another draft (the original per-draft bug).
    expect(selectedScope.value).toEqual({ kind: 'app', appId: 'a1' });
    expect(resolveScope('other-draft')).toEqual({ kind: 'lucidos' });
    expect(getDraft(id).mode).toBe('claude_code');
  });

  it('Lucidos Agent resets the channel but leaves the remembered scope alone', () => {
    const id = 'dest-agent';
    putThread(id, 'composing');
    setDraft(id, { text: 'fix it', image_hashes: [], mode: 'claude_code' });
    selectedScope.value = { kind: 'external', repoId: 'r1' };

    applyDestination(id, { kind: 'lucidos-agent' });

    expect(inputMode.value).toEqual({ type: 'do' });
    // Scope is the remembered coding target — switching to the agent must not
    // forget it, so flipping back lands on the same target.
    expect(selectedScope.value).toEqual({ kind: 'external', repoId: 'r1' });
    expect(getDraft(id).mode).toBe('lucidos');
  });

  it('no focused thread writes the pending slot AND the last-used scope seed, without leaking to existing drafts', () => {
    // selectedScope starts at {lucidos} (beforeEach). Pick a DIFFERENT target so
    // the writes are distinguishable.
    applyDestination(null, { kind: 'coding', scope: { kind: 'app', appId: 'a1' } });

    expect(inputMode.value).toEqual({ type: 'coding_agent' });
    // Pending slot captured the target (transferred onto the draft at creation)…
    expect(pendingComposeSelection.value.scope).toEqual({ kind: 'app', appId: 'a1' });
    // …and the localStorage last-used seed is updated so the fresh compose view
    // and the next new draft start from it. Leak-safe: an EXISTING draft resolves
    // its own scope (fixed default here), never this seed.
    expect(selectedScope.value).toEqual({ kind: 'app', appId: 'a1' });
    expect(resolveScope('existing-draft')).toEqual({ kind: 'lucidos' });
    expect(getDraft(null).mode).toBe(null);
  });

  it('an active (non-composing) thread never gets a draft patch', () => {
    const id = 'dest-active';
    putThread(id, 'active');
    setDraft(id, { text: '', image_hashes: [], mode: null });

    applyDestination(id, { kind: 'coding', scope: { kind: 'lucidos' } });

    expect(getDraft(id).mode).toBe(null);
  });

  it('re-picking the same destination is a no-op (no signal identity churn)', () => {
    // The Dropdown fires onChange even on a click of the already-selected
    // option — applyDestination must not rewrite signals (subscriber
    // re-renders) or re-patch the draft (debounced PUT + SSE fan-out).
    applyDestination(null, { kind: 'coding', scope: { kind: 'app', appId: 'a1' } });
    const pendingBefore = pendingComposeSelection.value;
    const modeBefore = inputMode.value;

    applyDestination(null, { kind: 'coding', scope: { kind: 'app', appId: 'a1' } });

    expect(pendingComposeSelection.value).toBe(pendingBefore);
    expect(inputMode.value).toBe(modeBefore);
  });

  it('null draft mode is patched even when the global mode already matches', () => {
    // null = the engine hasn't acked a pick yet; the patch locks it in.
    const id = 'dest-null-mode';
    inputMode.value = { type: 'coding_agent' };
    putThread(id, 'composing');
    setDraft(id, { text: 'x', image_hashes: [], mode: null });

    applyDestination(id, { kind: 'coding', scope: { kind: 'lucidos' } });

    expect(getDraft(id).mode).toBe('claude_code');
  });
});

describe('destinationCaption', () => {
  it('Lucidos Agent advertises the hand-off', () => {
    expect(destinationCaption({ kind: 'lucidos-agent' }))
      .toBe('Chat, research, and create apps & triggers — can hand off to a coding agent.');
  });

  it('Lucidos source promises a reviewable change', () => {
    expect(destinationCaption({ kind: 'coding', scope: { kind: 'lucidos' } }))
      .toContain('review & Apply the change');
  });

  it('app target names the app, falling back to its id', () => {
    const dest: ComposeDestination = { kind: 'coding', scope: { kind: 'app', appId: 'habit-tracker' } };
    expect(destinationCaption(dest, { appName: 'Habit Tracker' })).toContain('the Habit Tracker app');
    expect(destinationCaption(dest)).toContain('the habit-tracker app');
  });

  it('repository target reviews the diff from the thread — never promises Apply', () => {
    const dest: ComposeDestination = { kind: 'coding', scope: { kind: 'external', repoId: 'r1' } };
    const caption = destinationCaption(dest, { repoName: 'my-project' });
    expect(caption).toContain('my-project');
    expect(caption).toContain('review the diff from the thread');
    expect(caption).not.toContain('Apply');
    expect(destinationCaption(dest)).toContain('the repository');
  });
});

describe('composeDraftContextName', () => {
  const repos = [
    { id: 'r1', name: 'my-project' },
    { id: 'r2', name: 'another-repo' },
  ];

  it('chat draft has no context chip — same as a started chat thread', () => {
    expect(composeDraftContextName('lucidos', { kind: 'lucidos' }, repos)).toBeUndefined();
    expect(composeDraftContextName(null, { kind: 'lucidos' }, repos)).toBeUndefined();
    // A chat draft ignores any leftover coding scope.
    expect(composeDraftContextName('lucidos', { kind: 'external', repoId: 'r1' }, repos)).toBeUndefined();
  });

  it('Lucidos source coding draft chips "Lucidos" (matches the started thread)', () => {
    expect(composeDraftContextName('claude_code', { kind: 'lucidos' }, repos))
      .toBe(LUCIDOS_SOURCE_REPO_NAME);
  });

  it('app coding draft chips the app id (matches appIdFromFolder on started threads)', () => {
    expect(composeDraftContextName('claude_code', { kind: 'app', appId: 'habit-tracker' }, repos))
      .toBe('habit-tracker');
  });

  it('external coding draft resolves the repo id to its name', () => {
    expect(composeDraftContextName('claude_code', { kind: 'external', repoId: 'r2' }, repos))
      .toBe('another-repo');
  });

  it('external coding draft is undefined until the repos list resolves the id', () => {
    expect(composeDraftContextName('claude_code', { kind: 'external', repoId: 'r1' }, []))
      .toBeUndefined();
    expect(composeDraftContextName('claude_code', { kind: 'external', repoId: 'gone' }, repos))
      .toBeUndefined();
  });
});
