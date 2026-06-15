import { describe, it, expect, beforeEach, vi } from 'vitest';
import { threadMap, focusedThreadId, changes, confirmState, applyingNowThreadIds, discardingCCThreadIds, archivingThreadIds } from '../store';
import type { ThreadState } from '../thread-events';
import type { Change } from '../../api/client';
import { makeThreadState } from './threads-test-helpers';
import { _resetComposeDraftsForTesting, getDraft } from '../composeDrafts';
import { resolveThreadActions, resolveGlobalActions, nextCloseLayer, runCloseCascade, discardDraft, type TaggedAction } from './threadActions';
import {
  pushOverlay,
  removeOverlay,
  topOverlay,
  dismissTopOverlay,
  overlayStack,
  _resetOverlayStackForTesting,
} from '../overlayStack';

function setThread(state: ThreadState): void {
  threadMap.value = new Map([[state.meta.id, state]]);
}

function pendingChangeRow(threadId: string, extra: Partial<Change> = {}): Change {
  return {
    id: `change-${threadId}`,
    thread_id: threadId,
    status: 'pending',
    file_count: 1,
    requires_restart: false,
    incomplete: false,
    ...extra,
  } as unknown as Change;
}

function kinds(actions: TaggedAction[]): string[] {
  return actions.map((a) => a.kind);
}

beforeEach(() => {
  threadMap.value = new Map();
  focusedThreadId.value = null;
  changes.value = { status: 'loaded', data: [] };
  applyingNowThreadIds.value = new Map();
  discardingCCThreadIds.value = new Set();
  archivingThreadIds.value = new Set();
  confirmState.value = { visible: false, message: '', okLabel: 'Delete' };
  _resetComposeDraftsForTesting();
  _resetOverlayStackForTesting();
});

function ta(kind: TaggedAction['kind']): TaggedAction {
  return { kind, category: 'close', label: kind, invoke: () => {} };
}

describe('resolveThreadActions', () => {
  it('idle inbox chat → Archive (close) then Save (save)', () => {
    setThread(makeThreadState('t1', { meta: { section: 'inbox', status: 'idle' } }));
    const actions = resolveThreadActions('t1');
    expect(kinds(actions)).toEqual(['archive', 'save']);
    expect(actions[0]).toMatchObject({ category: 'close', label: 'Archive' });
    expect(actions[1]).toMatchObject({ category: 'save', label: 'Save' });
  });

  it('saved thread shows the Unsave toggle, not Save', () => {
    setThread(makeThreadState('t1', { meta: { section: 'inbox', status: 'idle', saved: true } }));
    const actions = resolveThreadActions('t1');
    expect(kinds(actions)).toEqual(['archive', 'unsave']);
    expect(actions[1]).toMatchObject({ category: 'save', label: '✓ Saved' });
  });

  it('CC thread with a pending change → Discard (close) + Apply (primary) + Save', () => {
    setThread(makeThreadState('t1', {
      meta: { channel: 'claude_code', section: 'inbox', status: 'idle', codingAgentProposed: true },
    }));
    changes.value = { status: 'loaded', data: [pendingChangeRow('t1')] };
    const actions = resolveThreadActions('t1');
    expect(kinds(actions)).toEqual(['discard', 'apply', 'save']);
    expect(actions.find((a) => a.kind === 'apply')).toMatchObject({ category: 'primary', label: 'Apply' });
    expect(actions.find((a) => a.kind === 'discard')).toMatchObject({ category: 'close', label: 'Discard' });
  });

  it('Apply label becomes "Apply & Restart" when the change requires restart', () => {
    setThread(makeThreadState('t1', {
      meta: { channel: 'claude_code', section: 'inbox', status: 'idle', codingAgentProposed: true },
    }));
    changes.value = { status: 'loaded', data: [pendingChangeRow('t1', { requires_restart: true })] };
    const apply = resolveThreadActions('t1').find((a) => a.kind === 'apply');
    expect(apply?.label).toBe('Apply & Restart');
  });

  it('external-repo CC swaps the change layer for Archive', () => {
    // External-repo: coding_agent_proposed=true but no pending changes row.
    setThread(makeThreadState('t1', {
      meta: {
        channel: 'claude_code',
        section: 'inbox',
        status: 'idle',
        codingAgentProposed: true,
        codingAgentIsExternalRepo: true,
      },
    }));
    const actions = resolveThreadActions('t1');
    expect(kinds(actions)).toEqual(['archive', 'save']);
  });

  it('an unsent draft is the front-most close action', () => {
    setThread(makeThreadState('t1', {
      meta: { section: 'inbox', status: 'idle', composeText: 'half a thought' },
    }));
    const actions = resolveThreadActions('t1');
    expect(kinds(actions)).toEqual(['discard_draft', 'archive', 'save']);
    expect(actions[0]).toMatchObject({ category: 'close', label: 'Discard draft' });
  });

  it('draft discard is offered even while the thread is live (mid-turn)', () => {
    setThread(makeThreadState('t1', {
      meta: { section: 'inbox', status: 'running', composeText: 'typing while it works' },
    }));
    expect(kinds(resolveThreadActions('t1'))).toEqual(['discard_draft', 'save']);
  });

  it('returns [] for an unknown thread', () => {
    expect(resolveThreadActions('nope')).toEqual([]);
  });
});

describe('overlayStack', () => {
  it('is LIFO; top reflects the most recent push', () => {
    pushOverlay({ id: 'a', dismiss: () => {} });
    pushOverlay({ id: 'b', dismiss: () => {} });
    expect(topOverlay()?.id).toBe('b');
    removeOverlay('b');
    expect(topOverlay()?.id).toBe('a');
  });

  it('dismissTopOverlay calls the top entry dismiss and reports whether one existed', () => {
    let dismissed = '';
    pushOverlay({ id: 'a', dismiss: () => { dismissed = 'a'; } });
    pushOverlay({ id: 'b', dismiss: () => { dismissed = 'b'; } });
    expect(dismissTopOverlay()).toBe(true);
    expect(dismissed).toBe('b');
    _resetOverlayStackForTesting();
    expect(dismissTopOverlay()).toBe(false);
  });

  it('re-pushing the same id replaces rather than duplicates', () => {
    pushOverlay({ id: 'a', dismiss: () => {} });
    pushOverlay({ id: 'a', dismiss: () => {} });
    expect(overlayStack.value.filter((e) => e.id === 'a')).toHaveLength(1);
  });
});

describe('resolveGlobalActions', () => {
  it('prepends a dismiss-overlay action when an overlay is open', () => {
    setThread(makeThreadState('t1', { meta: { section: 'inbox', status: 'idle' } }));
    focusedThreadId.value = 't1';
    pushOverlay({ id: 'modal', dismiss: () => {} });
    const actions = resolveGlobalActions();
    expect(actions[0]).toMatchObject({ kind: 'dismiss_overlay', category: 'dismiss' });
    // ...followed by the focused thread's actions.
    expect(kinds(actions).slice(1)).toEqual(['archive', 'save']);
  });

  it('with no overlay open, returns just the focused thread actions', () => {
    setThread(makeThreadState('t1', { meta: { section: 'inbox', status: 'idle' } }));
    focusedThreadId.value = 't1';
    expect(kinds(resolveGlobalActions())).toEqual(['archive', 'save']);
  });

  it('with no focused thread and no overlay, returns []', () => {
    expect(resolveGlobalActions()).toEqual([]);
  });
});

describe('nextCloseLayer', () => {
  it('draft is the front-most layer', () => {
    expect(nextCloseLayer([ta('discard_draft'), ta('archive'), ta('save')])).toBe('draft');
  });

  it('change layer (discard/apply) wins when no draft', () => {
    expect(nextCloseLayer([ta('discard'), ta('apply'), ta('save')])).toBe('change');
  });

  it('archive when no draft or change', () => {
    expect(nextCloseLayer([ta('archive'), ta('save')])).toBe('archive');
  });

  it('null when only the save toggle is available', () => {
    expect(nextCloseLayer([ta('save')])).toBeNull();
    expect(nextCloseLayer([])).toBeNull();
  });
});

describe('runCloseCascade no-op gates', () => {
  it('no-ops with no focused thread (does not open a confirm)', async () => {
    await runCloseCascade();
    expect(confirmState.value.visible).toBe(false);
  });

  it('no-ops while a layer is still resolving in-flight', async () => {
    // A thread with a pending change would normally open the change choice, but
    // an in-flight apply must gate the cascade to a no-op (async bridge).
    setThread(makeThreadState('t1', {
      meta: { channel: 'claude_code', section: 'inbox', status: 'idle', codingAgentProposed: true },
    }));
    changes.value = { status: 'loaded', data: [pendingChangeRow('t1')] };
    focusedThreadId.value = 't1';
    applyingNowThreadIds.value = new Map([['t1', 'applying']]);

    await runCloseCascade();
    expect(confirmState.value.visible).toBe(false);
  });

  it('opens the apply/discard choice for a thread with a pending change', async () => {
    setThread(makeThreadState('t1', {
      meta: { channel: 'claude_code', section: 'inbox', status: 'idle', codingAgentProposed: true },
    }));
    changes.value = { status: 'loaded', data: [pendingChangeRow('t1')] };
    focusedThreadId.value = 't1';

    // showConfirm's promise resolves on user interaction; we only assert the
    // dialog opened with the three-way choice, then dismiss it.
    void runCloseCascade();
    await Promise.resolve();
    expect(confirmState.value.visible).toBe(true);
    expect(confirmState.value.okLabel).toBe('Apply');
    expect(confirmState.value.extraAction?.label).toBe('Discard');
    confirmState.value.resolve?.(false);
  });
});

describe('discardDraft (confirm lives on the action)', () => {
  it('always opens the discard confirm before dropping anything', async () => {
    setThread(makeThreadState('t1', { meta: { section: 'inbox', status: 'idle', composeText: 'half a thought' } }));
    void discardDraft('t1');
    await Promise.resolve();
    expect(confirmState.value.visible).toBe(true);
    expect(confirmState.value.message).toBe('Discard this unsent draft?');
    expect(confirmState.value.okLabel).toBe('Discard');
    confirmState.value.resolve?.(false);
  });

  it('returns false and keeps the draft when the user cancels', async () => {
    setThread(makeThreadState('t1', { meta: { state: 'active', section: 'inbox', status: 'idle', composeText: 'keep me' } }));
    const p = discardDraft('t1');
    await Promise.resolve();
    confirmState.value.resolve?.(false);
    expect(await p).toBe(false);
    expect(getDraft('t1').text).toBe('keep me');
  });

  it('returns true and clears the draft when the user confirms', async () => {
    vi.useFakeTimers();
    try {
      setThread(makeThreadState('t1', { meta: { state: 'active', section: 'inbox', status: 'idle', composeText: 'drop me' } }));
      const p = discardDraft('t1');
      await Promise.resolve();
      confirmState.value.resolve?.(true);
      expect(await p).toBe(true);
      expect(getDraft('t1').text).toBe('');
    } finally {
      // updateCompose schedules a debounced sync PUT; drop it so the real
      // timer can't fire fetch after the test.
      vi.clearAllTimers();
      vi.useRealTimers();
    }
  });

  it('the discard_draft action routes through the same confirm as the cascade', async () => {
    // The close-cascade shortcut drives this TaggedAction's invoke (the 'draft'
    // close layer) — so the confirm lives on the action, not on any one caller.
    setThread(makeThreadState('t1', { meta: { section: 'inbox', status: 'idle', composeText: 'half a thought' } }));
    const action = resolveThreadActions('t1').find((a) => a.kind === 'discard_draft');
    void action?.invoke();
    await Promise.resolve();
    expect(confirmState.value.visible).toBe(true);
    expect(confirmState.value.okLabel).toBe('Discard');
    confirmState.value.resolve?.(false);
  });
});
