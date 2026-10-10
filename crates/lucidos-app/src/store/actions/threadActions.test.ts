import { describe, it, expect, beforeEach, vi } from 'vitest';
import { threadMap, focusedThreadId, changes, confirmState, applyingNowThreadIds, discardingCCThreadIds, archivingThreadIds, armingStandingApplyThreadIds } from '../store';
import type { ThreadMeta, ThreadState } from '../thread-events';
import type { Change } from '../../api/client';
import { makeThreadState } from './threads-test-helpers';
import { _resetComposeDraftsForTesting, getDraft } from '../composeDrafts';
import { resolveThreadActions, resolveChangeMenuActions, nextCloseLayer, runCloseCascade, discardDraft, type TaggedAction } from './threadActions';
import { ARCHIVE_PINNED_CONFIRM } from './threads';
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
  armingStandingApplyThreadIds.value = new Set();
  confirmState.value = { visible: false, message: '', okLabel: 'Delete' };
  _resetComposeDraftsForTesting();
  _resetOverlayStackForTesting();
});

function ta(kind: TaggedAction['kind']): TaggedAction {
  return { kind, category: 'close', label: kind, invoke: () => {} };
}

describe('resolveThreadActions', () => {
  it('idle inbox chat → Archive (close) then Pin (save)', () => {
    setThread(makeThreadState('t1', { meta: { section: 'inbox', status: 'idle' } }));
    const actions = resolveThreadActions('t1');
    expect(kinds(actions)).toEqual(['archive', 'save']);
    expect(actions[0]).toMatchObject({ category: 'close', label: 'Archive' });
    expect(actions[1]).toMatchObject({ category: 'save', label: 'Pin' });
  });

  it('pinned thread shows the Unpin toggle, not Pin', () => {
    setThread(makeThreadState('t1', { meta: { section: 'inbox', status: 'idle', saved: true } }));
    const actions = resolveThreadActions('t1');
    expect(kinds(actions)).toEqual(['archive', 'unsave']);
    expect(actions[1]).toMatchObject({ category: 'save', label: '✓ Pinned' });
  });

  // A pinned thread always sits in the inbox (ADR 0312). So a settled one is
  // offered Archive, and the user is never left with Unpin alone.
  it('a pinned, settled coding-agent thread with no change offers Archive beside Unpin', () => {
    setThread(makeThreadState('t1', { meta: { channel: 'claude_code', section: 'inbox', status: 'idle', saved: true } }));
    expect(kinds(resolveThreadActions('t1'))).toEqual(['archive', 'unsave']);
  });

  // ADR 0395: a coding agent parked on an event wait with an unproposed diff
  // holds its proposal. That work outranks Archive the way a proposed change
  // did, so the thread keeps only the standing apply and the pin.
  it('a parked coding-agent thread with held work is not offered Archive', () => {
    setThread(makeThreadState('t1', {
      meta: { channel: 'claude_code', section: 'inbox', status: 'idle', codingAgentChangeState: { kind: 'unproposed', reason: null }, liveEventWaitCount: 1 },
    }));
    expect(kinds(resolveThreadActions('t1'))).toEqual(['apply_when_settled', 'save']);
  });

  it('a parked external-repo coding-agent thread with a diff keeps Archive', () => {
    setThread(makeThreadState('t1', {
      meta: { channel: 'claude_code', section: 'inbox', status: 'idle', codingAgentKind: 'external', codingAgentChangeState: { kind: 'unproposed', reason: null }, liveEventWaitCount: 1 },
    }));
    expect(kinds(resolveThreadActions('t1'))).toContain('archive');
  });

  it('a parked coding-agent thread with no diff keeps Archive', () => {
    setThread(makeThreadState('t1', {
      meta: { channel: 'claude_code', section: 'inbox', status: 'idle', codingAgentChangeState: { kind: 'none' }, liveEventWaitCount: 1 },
    }));
    expect(kinds(resolveThreadActions('t1'))).toContain('archive');
  });

  it('CC thread with a pending change → Discard (close) + Apply (primary) + Set aside + Pin', () => {
    setThread(makeThreadState('t1', {
      meta: { channel: 'claude_code', section: 'inbox', status: 'idle', codingAgentChangeState: { kind: 'proposed', requires_restart: false }},
    }));
    changes.value = { status: 'loaded', data: [pendingChangeRow('t1')] };
    const actions = resolveThreadActions('t1');
    expect(kinds(actions)).toEqual(['discard', 'apply', 'set_aside', 'save']);
    expect(actions.find((a) => a.kind === 'apply')).toMatchObject({ category: 'primary', label: 'Apply' });
    expect(actions.find((a) => a.kind === 'discard')).toMatchObject({ category: 'close', label: 'Discard' });
    expect(actions.find((a) => a.kind === 'set_aside')).toMatchObject({ category: 'close', label: 'Set aside' });
  });

  // Set aside acts on one change by id. A proposal flag can land before the
  // pending list, and then Apply and Discard show but Set aside waits.
  it('offers no Set aside until the pending list names the change', () => {
    setThread(makeThreadState('t1', {
      meta: { channel: 'claude_code', section: 'inbox', status: 'idle', codingAgentChangeState: { kind: 'proposed', requires_restart: false }},
    }));
    changes.value = { status: 'loaded', data: [] };
    expect(kinds(resolveThreadActions('t1'))).toEqual(['discard', 'apply', 'save']);
  });

  // A delegating parent: idle apart from a running sub-thread. The child
  // writes its own worktree, so the parent's own change is resolvable (ADR
  // 0249). A live event wait still withholds both, and offers the standing
  // apply in their place: the wait ends by itself (ADR 0266).
  it('a running sub-thread does not withhold Apply or Discard; an event wait swaps them for the standing apply', () => {
    const parent = makeThreadState('t1', {
      meta: { channel: 'claude_code', section: 'inbox', status: 'idle', codingAgentChangeState: { kind: 'proposed', requires_restart: false }, activeChildrenCount: 2 },
    });
    setThread(parent);
    changes.value = { status: 'loaded', data: [pendingChangeRow('t1')] };
    expect(kinds(resolveThreadActions('t1'))).toEqual(['discard', 'apply', 'set_aside', 'save']);

    setThread(makeThreadState('t1', {
      meta: { ...parent.meta, liveEventWaitCount: 1 },
    }));
    expect(kinds(resolveThreadActions('t1'))).toEqual(['apply_when_settled', 'save']);
  });

  it('Apply gets a compact "Apply*" marker for a restart-requiring change (restart is the separate switch)', () => {
    setThread(makeThreadState('t1', {
      meta: { channel: 'claude_code', section: 'inbox', status: 'idle', codingAgentChangeState: { kind: 'proposed', requires_restart: false }},
    }));
    changes.value = { status: 'loaded', data: [pendingChangeRow('t1', { requires_restart: true })] };
    const apply = resolveThreadActions('t1').find((a) => a.kind === 'apply');
    // Apply is always non-disruptive now — the "Apply & Restart" dual label
    // stays retired. requiresRestart surfaces as a compact "Apply*" marker plus
    // the tooltip (the new engine version builds in the background; the user
    // switches to it separately).
    expect(apply?.label).toBe('Apply*');
    expect(apply?.tooltip).toContain('new engine version');
  });

  it('external-repo CC swaps the change layer for Archive', () => {
    // External-repo: the thread reads proposed but has no pending changes row.
    setThread(makeThreadState('t1', {
      meta: {
        channel: 'claude_code',
        section: 'inbox',
        status: 'idle',
        codingAgentChangeState: { kind: 'proposed', requires_restart: false },
        codingAgentIsExternalRepo: true,
      },
    }));
    const actions = resolveThreadActions('t1');
    expect(kinds(actions)).toEqual(['archive', 'save']);
  });

  // The standing apply is an Apply the owner presses early, so it belongs
  // wherever Apply does. Lucidos never merges into an external repo, and such a
  // thread proposes nothing, so the flag offered to apply a change that could
  // never exist.
  //
  // Both statuses matter: those are the two the core offers the action in. Both
  // facts matter too: `codingAgentKind` is what a live thread carries, and the
  // legacy bool is what an old row carries instead.
  describe('the standing apply is withheld where Lucidos never applies', () => {
    const externalFacts: [string, Partial<ThreadMeta>][] = [
      ['by coding-agent kind', { codingAgentKind: 'external' }],
      ['by the legacy external-repo flag', { codingAgentIsExternalRepo: true }],
    ];
    for (const status of ['running', 'paused'] as const) {
      for (const [label, meta] of externalFacts) {
        it(`withholds it from a ${status} external-repo thread, ${label}`, () => {
          setThread(makeThreadState('t1', {
            meta: { channel: 'claude_code', section: 'inbox', status, ...meta },
          }));
          expect(kinds(resolveThreadActions('t1'))).not.toContain('apply_when_settled');
        });
      }

      it(`still offers it on a ${status} Lucidos-source thread`, () => {
        setThread(makeThreadState('t1', {
          meta: { channel: 'claude_code', section: 'inbox', status, codingAgentKind: 'lucidos' },
        }));
        expect(kinds(resolveThreadActions('t1'))).toContain('apply_when_settled');
      });
    }

    // An app thread merges into the workspace git, so Lucidos does apply it.
    it('still offers it on an app thread, which Lucidos does apply', () => {
      setThread(makeThreadState('t1', {
        meta: {
          channel: 'claude_code',
          section: 'inbox',
          status: 'running',
          codingAgentKind: 'app',
        },
      }));
      expect(kinds(resolveThreadActions('t1'))).toContain('apply_when_settled');
    });

    // The regression: `getCodingAgentWaitingInfo` returns null for a running
    // thread, so a carve-out reading it could never fire where the flag lives.
    it('withholds it before the thread has ever proposed anything', () => {
      setThread(makeThreadState('t1', {
        meta: {
          channel: 'claude_code',
          section: 'inbox',
          status: 'running',
          codingAgentChangeState: { kind: 'none' },
          codingAgentKind: 'external',
        },
      }));
      expect(kinds(resolveThreadActions('t1'))).toEqual(['save']);
    });
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

  it('composing draft does not offer the save toggle', () => {
    // A composing draft is excluded from every drawer section (including Pinned),
    // so saving it would set is_saved but leave the row in the compose surface.
    setThread(makeThreadState('t1', {
      meta: { state: 'composing', section: 'inbox', status: 'idle', composeText: 'half a thought' },
    }));
    const actions = resolveThreadActions('t1');
    expect(kinds(actions)).toEqual(['discard_draft', 'archive']);
    expect(kinds(actions)).not.toContain('save');
    expect(kinds(actions)).not.toContain('unsave');
  });

  it('returns [] for an unknown thread', () => {
    expect(resolveThreadActions('nope')).toEqual([]);
  });
});

describe('resolveChangeMenuActions', () => {
  function proposedCcThread(extra: Partial<ThreadMeta> = {}): void {
    setThread(makeThreadState('t1', {
      meta: { channel: 'claude_code', section: 'inbox', status: 'idle', codingAgentChangeState: { kind: 'proposed', requires_restart: false }, ...extra },
    }));
    changes.value = { status: 'loaded', data: [pendingChangeRow('t1')] };
  }

  it('lists Apply, then Set aside, then Discard for a pending change, and nothing else', () => {
    proposedCcThread();
    expect(kinds(resolveChangeMenuActions('t1'))).toEqual(['apply', 'set_aside', 'discard']);
  });

  it('offers the standing apply while an event wait withholds Apply', () => {
    proposedCcThread({ liveEventWaitCount: 1 });
    expect(kinds(resolveChangeMenuActions('t1'))).toEqual(['apply_when_settled']);
  });

  it('drops the standing apply while its own request is in flight', () => {
    proposedCcThread({ liveEventWaitCount: 1 });
    armingStandingApplyThreadIds.value = new Set(['t1']);
    expect(resolveChangeMenuActions('t1')).toEqual([]);
  });

  it('is empty for a thread with no change', () => {
    setThread(makeThreadState('t1', { meta: { section: 'inbox', status: 'idle' } }));
    expect(resolveChangeMenuActions('t1')).toEqual([]);
  });

  it('is empty while an apply is in flight', () => {
    proposedCcThread();
    applyingNowThreadIds.value = new Map([['t1', 'applying']]);
    expect(resolveChangeMenuActions('t1')).toEqual([]);
  });

  it('is empty while a discard is in flight', () => {
    proposedCcThread();
    discardingCCThreadIds.value = new Set(['t1']);
    expect(resolveChangeMenuActions('t1')).toEqual([]);
  });
});

describe('overlayStack', () => {
  it('is LIFO; top reflects the most recent push', () => {
    pushOverlay({ id: 'a', dismiss: () => {}, hasPanel: true });
    pushOverlay({ id: 'b', dismiss: () => {}, hasPanel: true });
    expect(topOverlay()?.id).toBe('b');
    removeOverlay('b');
    expect(topOverlay()?.id).toBe('a');
  });

  it('dismissTopOverlay calls the top entry dismiss and reports whether one existed', () => {
    let dismissed = '';
    pushOverlay({ id: 'a', dismiss: () => { dismissed = 'a'; }, hasPanel: true });
    pushOverlay({ id: 'b', dismiss: () => { dismissed = 'b'; }, hasPanel: true });
    expect(dismissTopOverlay()).toBe(true);
    expect(dismissed).toBe('b');
    _resetOverlayStackForTesting();
    expect(dismissTopOverlay()).toBe(false);
  });

  it('re-pushing the same id replaces rather than duplicates', () => {
    pushOverlay({ id: 'a', dismiss: () => {}, hasPanel: true });
    pushOverlay({ id: 'a', dismiss: () => {}, hasPanel: true });
    expect(overlayStack.value.filter((e) => e.id === 'a')).toHaveLength(1);
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
      meta: { channel: 'claude_code', section: 'inbox', status: 'idle', codingAgentChangeState: { kind: 'proposed', requires_restart: false }},
    }));
    changes.value = { status: 'loaded', data: [pendingChangeRow('t1')] };
    focusedThreadId.value = 't1';
    applyingNowThreadIds.value = new Map([['t1', 'applying']]);

    await runCloseCascade();
    expect(confirmState.value.visible).toBe(false);
  });

  it('archiving a pinned thread asks first, saying that it unpins', async () => {
    setThread(makeThreadState('t1', { meta: { section: 'inbox', status: 'idle', saved: true } }));
    focusedThreadId.value = 't1';

    const pending = runCloseCascade();
    await Promise.resolve();
    expect(confirmState.value.visible).toBe(true);
    expect(confirmState.value.message).toBe(ARCHIVE_PINNED_CONFIRM);
    expect(confirmState.value.okLabel).toBe('Archive');
    confirmState.value.resolve?.(false);
    await pending;
  });

  it('opens the apply/discard choice for a thread with a pending change', async () => {
    setThread(makeThreadState('t1', {
      meta: { channel: 'claude_code', section: 'inbox', status: 'idle', codingAgentChangeState: { kind: 'proposed', requires_restart: false }},
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
