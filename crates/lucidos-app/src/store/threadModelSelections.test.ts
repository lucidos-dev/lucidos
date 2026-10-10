/** Per-thread model/effort memory: a thread resolves `pending pick ?? its last
 *  message ?? the account default`, a pick on one thread never touches another
 *  or the account preference, and clearing drops back to the message-derived
 *  value. These pin the "this thread only" + "backend-authoritative fallback"
 *  decisions from docs/plans/2026-07-03-per-thread-model-memory.md. */
import { afterEach, describe, expect, it } from 'vitest';
import {
  _resetThreadModelSelectionsForTesting,
  clearThreadModelOverride,
  getThreadModelOverride,
  lastThreadModel,
  patchThreadModelOverride,
  resolveActiveThreadModel,
  resolveActiveThreadProvider,
  resolveActiveThreadReasoningEffort,
} from './threadModelSelections';
import { currentModel, preferences, threadMap } from './store';
import type { StoredEvent } from './thread-events';
import type { ThreadState } from './thread-events';

/** Minimal ThreadState carrying only the events the resolvers read. */
function seedThread(id: string, events: Array<[number, Partial<StoredEvent>]>): void {
  const map = new Map<number, StoredEvent>(events.map(([seq, e]) => [seq, e as StoredEvent]));
  const next = new Map(threadMap.value);
  next.set(id, { events: map } as unknown as ThreadState);
  threadMap.value = next;
}

/** The account's stored tier for `account-model`. */
const ACCOUNT_EFFORTS = 'account-model=high';

function resetAccount(): void {
  currentModel.value = 'account-model';
  preferences.value = { status: 'loaded', data: { chat_reasoning_efforts: ACCOUNT_EFFORTS } };
}

resetAccount();

afterEach(() => {
  _resetThreadModelSelectionsForTesting();
  threadMap.value = new Map();
  resetAccount();
});

describe('resolveActiveThreadModel: pending ?? last message ?? account default', () => {
  it('falls back to the account default with no override and no messages', () => {
    currentModel.value = 'account-model';
    expect(resolveActiveThreadModel('t-1')).toBe('account-model');
    expect(resolveActiveThreadModel(null)).toBe('account-model');
    expect(resolveActiveThreadModel(undefined)).toBe('account-model');
  });

  it("uses the thread's most recent MessageReceived model over the account default", () => {
    currentModel.value = 'account-model';
    seedThread('t-1', [
      [1, { type: 'MessageReceived', model: 'old-model' }],
      [2, { type: 'ResponseGenerated' }],
      [3, { type: 'MessageReceived', model: 'newer-model' }],
    ]);
    expect(resolveActiveThreadModel('t-1')).toBe('newer-model');
  });

  it('ignores MessageReceived rows without a model (synthetic failed sends)', () => {
    currentModel.value = 'account-model';
    seedThread('t-1', [
      [1, { type: 'MessageReceived', model: 'real-model' }],
      // a later synthetic failed-send row carries no model → must not win
      [-99, { type: 'MessageReceived', text: 'failed' }],
    ]);
    expect(resolveActiveThreadModel('t-1')).toBe('real-model');
  });

  it("uses a trigger thread's TriggerStarted model, which is its only starter event", () => {
    // A trigger fire emits TriggerStarted and no MessageReceived at all, so
    // reading only MessageReceived would show the account model here however
    // the trigger was pinned. Mirrors the backend's
    // `IN ('MessageReceived', 'TriggerStarted')`.
    seedThread('t-trigger', [
      [1, { type: 'TriggerStarted', model: 'gemini-3.5-flash', reasoning_effort: 'low' }],
      [2, { type: 'ResponseGenerated' }],
    ]);
    expect(resolveActiveThreadModel('t-trigger')).toBe('gemini-3.5-flash');
    expect(resolveActiveThreadReasoningEffort('t-trigger', 'gemini-3.5-flash')).toBe('low');
  });

  it('a legacy TriggerStarted without a model still falls back to the account default', () => {
    currentModel.value = 'account-model';
    seedThread('t-legacy', [[1, { type: 'TriggerStarted', trigger_id: 't-1' }]]);
    expect(resolveActiveThreadModel('t-legacy')).toBe('account-model');
  });

  it('a pending pick wins over both the last message and the account default', () => {
    currentModel.value = 'account-model';
    seedThread('t-1', [[1, { type: 'MessageReceived', model: 'last-model' }]]);
    patchThreadModelOverride('t-1', { model: 'picked-model' });
    expect(resolveActiveThreadModel('t-1')).toBe('picked-model');
  });
});

describe('resolveActiveThreadReasoningEffort: pending ?? memory for the model ?? account', () => {
  it('last tier on the same model ?? account tier; pending pick wins', () => {
    expect(resolveActiveThreadReasoningEffort('t-1', 'account-model')).toBe('high');
    seedThread('t-1', [[1, { type: 'MessageReceived', model: 'account-model', reasoning_effort: 'low' }]]);
    expect(resolveActiveThreadReasoningEffort('t-1', 'account-model')).toBe('low');
    patchThreadModelOverride('t-1', { reasoningEffort: 'max' });
    expect(resolveActiveThreadReasoningEffort('t-1', 'account-model')).toBe('max');
  });

  it('a thread that stamped no model keeps its tier on the model it runs on', () => {
    seedThread('t-1', [[1, { type: 'TriggerStarted', reasoning_effort: 'max' }]]);
    expect(resolveActiveThreadReasoningEffort('t-1', 'account-model')).toBe('max');
    expect(resolveActiveThreadReasoningEffort('t-1', 'other-model')).toBeNull();
  });

  it('a model-less thread tier stops once the account names a chat model', () => {
    seedThread('t-1', [[1, { type: 'TriggerStarted', reasoning_effort: 'max' }]]);
    preferences.value = {
      status: 'loaded',
      data: { chat_model: 'account-model', chat_reasoning_efforts: ACCOUNT_EFFORTS },
    };
    expect(resolveActiveThreadReasoningEffort('t-1', 'account-model')).toBe('high');
  });

  it('a remembered tier never follows the thread to another model', () => {
    seedThread('t-1', [[1, { type: 'MessageReceived', model: 'account-model', reasoning_effort: 'low' }]]);
    expect(resolveActiveThreadReasoningEffort('t-1', 'other-model')).toBeNull();
  });

  it('lastThreadModel returns undefined for an unknown thread', () => {
    expect(lastThreadModel('nope')).toBeUndefined();
    expect(lastThreadModel(null)).toBeUndefined();
  });
});

describe('a pick is per-thread and never writes the account preference', () => {
  it('picking on thread A leaves thread B and the account default untouched', () => {
    patchThreadModelOverride('A', { model: 'A-model', reasoningEffort: 'low' });
    // Thread B (no override, no messages) still resolves the account default.
    expect(resolveActiveThreadModel('B')).toBe('account-model');
    expect(resolveActiveThreadReasoningEffort('B', 'account-model')).toBe('high');
    // The account preference is untouched.
    expect(currentModel.value).toBe('account-model');
    expect(preferences.value).toEqual({ status: 'loaded', data: { chat_reasoning_efforts: ACCOUNT_EFFORTS } });
  });

  it('a later patch preserves earlier fields', () => {
    patchThreadModelOverride('t-1', { model: 'm' });
    patchThreadModelOverride('t-1', { reasoningEffort: 'e' });
    expect(getThreadModelOverride('t-1')).toEqual({ model: 'm', reasoningEffort: 'e' });
  });
});

describe('clearThreadModelOverride', () => {
  it('drops the pending pick so resolution falls to the last message', () => {
    currentModel.value = 'account-model';
    seedThread('t-1', [[1, { type: 'MessageReceived', model: 'last-model' }]]);
    patchThreadModelOverride('t-1', { model: 'picked-model' });
    expect(resolveActiveThreadModel('t-1')).toBe('picked-model');
    clearThreadModelOverride('t-1');
    expect(resolveActiveThreadModel('t-1')).toBe('last-model');
  });

  it('is a no-op for an unknown / null thread', () => {
    expect(() => clearThreadModelOverride('none')).not.toThrow();
    expect(() => clearThreadModelOverride(null)).not.toThrow();
  });
});

describe('resolveActiveThreadProvider: a backend is remembered with its model', () => {
  it("reads the thread's pick for the model it runs on, and drops it on a switch", () => {
    seedThread('t-1', [
      [1, { type: 'MessageReceived', model: 'opus', provider: 'vertex' }],
      [2, { type: 'MessageReceived', model: 'sonnet' }],
    ]);
    expect(resolveActiveThreadProvider('t-1', 'opus')).toBe('vertex');
    expect(resolveActiveThreadProvider('t-1', 'sonnet')).toBeNull();
  });

  it("prefers this thread's pending pick for the same model", () => {
    seedThread('t-1', [[1, { type: 'MessageReceived', model: 'opus', provider: 'vertex' }]]);
    patchThreadModelOverride('t-1', { model: 'opus', provider: 'anthropic' });
    expect(resolveActiveThreadProvider('t-1', 'opus')).toBe('anthropic');
    // Another thread never sees it.
    expect(resolveActiveThreadProvider('t-2', 'opus')).toBeNull();
  });
});
