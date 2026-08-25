import { describe, it, expect, beforeEach, vi } from 'vitest';
import { triggers, historicalTriggers, selectedTriggerIds, panelOverlay, toasts, triggerScrollTarget } from '../store';
import { makeTrigger } from '../__tests__/fixtures';

const STORAGE_KEY = 'lucidos-selected-trigger-ids';
const SCROLL_KEY = 'lucidos-scroll-content-triggers';

const mockCreateTrigger = vi.fn();
const mockUpdateTrigger = vi.fn();
const mockListTriggers = vi.fn();
vi.mock('../../api/client', () => ({
  createTrigger: (...args: unknown[]) => mockCreateTrigger(...args),
  updateTrigger: (...args: unknown[]) => mockUpdateTrigger(...args),
  listTriggers: (...args: unknown[]) => mockListTriggers(...args),
  listHistoricalTriggers: vi.fn().mockResolvedValue({ triggers: [] }),
  deleteTriggerApi: vi.fn(),
}));

// Pane helper: navigateToTrigger must call revealContentPane (the canonical
// "reveal content pane" helper that swipes on mobile AND expands a collapsed
// split on desktop), NOT an ad-hoc isMobile()+navigateToPane('content') that
// only handles mobile. See `.claude/rules/frontend.md`.
// Use vi.hoisted to declare the spy BEFORE vi.mock's hoisted call needs it.
const { mockRevealContentPane, mockSetActiveMenu } = vi.hoisted(() => ({
  mockRevealContentPane: vi.fn(),
  mockSetActiveMenu: vi.fn(),
}));
vi.mock('./pane', () => ({ revealContentPane: mockRevealContentPane, navigateToPane: vi.fn() }));

vi.mock('./menu', () => ({ setActiveMenu: mockSetActiveMenu }));
vi.mock('./navigation', () => ({ pushNavState: vi.fn() }));

const { pruneStaleSelectedTriggerIds, submitTrigger, navigateToTrigger } = await import('./triggers');

describe('pruneStaleSelectedTriggerIds', () => {
  beforeEach(() => {
    localStorage.clear();
    triggers.value = { status: 'not-loaded' };
    historicalTriggers.value = { status: 'not-loaded' };
    selectedTriggerIds.value = new Set();
  });

  it('does nothing while either registry is still loading', () => {
    selectedTriggerIds.value = new Set(['stale-id']);
    triggers.value = { status: 'loaded', data: [] };
    historicalTriggers.value = { status: 'loading' };

    pruneStaleSelectedTriggerIds();

    expect([...selectedTriggerIds.value]).toEqual(['stale-id']);
  });

  it('drops selected ids that match neither live nor historical, persists to localStorage', () => {
    selectedTriggerIds.value = new Set(['live-keep', 'hist-keep', 'stale-v5-hash']);
    triggers.value = { status: 'loaded', data: [makeTrigger({ id: 'live-keep', name: 'Live' })] };
    historicalTriggers.value = {
      status: 'loaded',
      data: [{ id: 'hist-keep', name: 'Hist', last_activity: '2026-04-30T00:00:00Z' }],
    };

    pruneStaleSelectedTriggerIds();

    expect([...selectedTriggerIds.value].sort()).toEqual(['hist-keep', 'live-keep']);
    expect(JSON.parse(localStorage.getItem(STORAGE_KEY) || '[]').sort()).toEqual(['hist-keep', 'live-keep']);
  });

  it('leaves selection untouched when every selected id is still valid', () => {
    selectedTriggerIds.value = new Set(['live-1']);
    triggers.value = { status: 'loaded', data: [makeTrigger({ id: 'live-1', name: 'Live' })] };
    historicalTriggers.value = { status: 'loaded', data: [] };

    pruneStaleSelectedTriggerIds();

    expect([...selectedTriggerIds.value]).toEqual(['live-1']);
    // No write — `setSelectedTriggerIds` only fires when something dropped.
    expect(localStorage.getItem(STORAGE_KEY)).toBeNull();
  });
});

describe('submitTrigger scroll reset', () => {
  beforeEach(() => {
    localStorage.clear();
    panelOverlay.value = { type: 'form', form: { type: 'trigger', triggerId: 't1' } };
    triggers.value = { status: 'loaded', data: [] };
    historicalTriggers.value = { status: 'loaded', data: [] };
    mockCreateTrigger.mockReset();
    mockUpdateTrigger.mockReset();
    mockListTriggers.mockReset().mockResolvedValue({ triggers: [] });
  });

  it('drops the saved trigger list scroll on update so the list returns to top', async () => {
    // Bug: editing a trigger far down a long list, saving, returning landed
    // back at the row instead of the top — useScrollMemory restored the
    // pre-edit scroll position.
    localStorage.setItem(SCROLL_KEY, '500');
    mockUpdateTrigger.mockResolvedValue({ success: true });

    const ok = await submitTrigger({
      name: 'Nightly Build',
      run: { type: 'intent', intent: 'build' },
      cronExpressions: ['0 0 0 * * *'],
      triggerId: 't1',
      goToReview: false,
      sideEffectGrant: [],
      model: null,
      reasoningEffort: null,
    });

    expect(ok).toBe(true);
    expect(localStorage.getItem(SCROLL_KEY)).toBeNull();
  });

  it('drops the saved trigger list scroll on create as well', async () => {
    localStorage.setItem(SCROLL_KEY, '300');
    mockCreateTrigger.mockResolvedValue({ success: true });

    const ok = await submitTrigger({
      name: 'New Trigger',
      run: { type: 'intent', intent: 'do thing' },
      cronExpressions: ['0 0 0 * * *'],
      goToReview: false,
      sideEffectGrant: [],
      model: null,
      reasoningEffort: null,
    });

    expect(ok).toBe(true);
    expect(localStorage.getItem(SCROLL_KEY)).toBeNull();
  });

  it('leaves the saved scroll alone when save fails (user stays on form)', async () => {
    localStorage.setItem(SCROLL_KEY, '500');
    mockUpdateTrigger.mockResolvedValue({ success: false, error: 'nope' });

    const ok = await submitTrigger({
      name: 'X',
      run: { type: 'intent', intent: 'x' },
      cronExpressions: ['0 0 0 * * *'],
      triggerId: 't1',
      goToReview: false,
      sideEffectGrant: [],
      model: null,
      reasoningEffort: null,
    });

    expect(ok).toBe(false);
    // Form is still open; the user hasn't navigated, so the saved position
    // must be preserved for the eventual successful save (or cancel).
    expect(localStorage.getItem(SCROLL_KEY)).toBe('500');
  });
});

describe('submitTrigger: the trigger model and reasoning effort', () => {
  beforeEach(() => {
    localStorage.clear();
    panelOverlay.value = { type: 'form', form: { type: 'trigger', triggerId: 't1' } };
    triggers.value = { status: 'loaded', data: [] };
    historicalTriggers.value = { status: 'loaded', data: [] };
    mockCreateTrigger.mockReset().mockResolvedValue({ success: true });
    mockUpdateTrigger.mockReset().mockResolvedValue({ success: true });
    mockListTriggers.mockReset().mockResolvedValue({ triggers: [] });
  });

  const intentRun = { type: 'intent', intent: 'summarize' } as const;
  const base = {
    name: 'Digest',
    cronExpressions: ['0 0 8 * * *'],
    goToReview: false,
    sideEffectGrant: [],
  };

  it('sends the pinned pair on create and omits it when Default', async () => {
    await submitTrigger({
      ...base, run: intentRun, model: 'gemini-3.5-flash', reasoningEffort: 'low',
    });
    expect(mockCreateTrigger.mock.calls[0][0]).toMatchObject({
      model: 'gemini-3.5-flash',
      reasoning_effort: 'low',
    });

    mockCreateTrigger.mockClear();
    await submitTrigger({ ...base, run: intentRun, model: null, reasoningEffort: null });
    const body = mockCreateTrigger.mock.calls[0][0];
    // Omitted, not null: a brand-new trigger has no stored pin to clear, so the
    // payload stays exactly what it was before the field existed.
    expect(body.model).toBeUndefined();
    expect(body.reasoning_effort).toBeUndefined();
  });

  it('sends null on update so switching back to Default clears the stored pin', async () => {
    // Omitting the field would mean "leave unchanged" to the engine, which
    // would silently keep the old model after the user chose Default.
    await submitTrigger({
      ...base, run: intentRun, triggerId: 't1', model: null, reasoningEffort: null,
    });
    const body = mockUpdateTrigger.mock.calls[0][1];
    expect(body.model).toBeNull();
    expect(body.reasoning_effort).toBeNull();
  });

  it('forces both to null for a script trigger, which runs no LLM', async () => {
    // Guards the intent → script switch: the form still holds the model the
    // user picked while it was an intent trigger, and none of it applies now.
    await submitTrigger({
      ...base,
      run: { type: 'script', path: 'digest/run.py' },
      triggerId: 't1',
      goToReview: true,
      sideEffectGrant: ['email'],
      model: 'gemini-3.5-flash',
      reasoningEffort: 'low',
    });
    const body = mockUpdateTrigger.mock.calls[0][1];
    expect(body.model).toBeNull();
    expect(body.reasoning_effort).toBeNull();
    expect(body.go_to_review).toBe(false);
    expect(body.side_effect_grant).toEqual([]);
  });
});

describe('submitTrigger surfaces the engine warnings', () => {
  beforeEach(() => {
    localStorage.clear();
    toasts.value = [];
    panelOverlay.value = { type: 'form', form: { type: 'trigger', triggerId: 't1' } };
    triggers.value = { status: 'loaded', data: [] };
    historicalTriggers.value = { status: 'loaded', data: [] };
    mockCreateTrigger.mockReset();
    mockUpdateTrigger.mockReset();
    mockListTriggers.mockReset().mockResolvedValue({ triggers: [] });
  });

  const base = {
    name: 'Digest',
    run: { type: 'intent', intent: 'summarize' } as const,
    cronExpressions: ['0 0 8 * * *'],
    goToReview: false,
    sideEffectGrant: [],
    model: null,
    reasoningEffort: null,
  };

  it('toasts an event-type warning on create, so a typo is caught at save time', async () => {
    // A trigger armed on a type nobody has emitted looks armed and never fires.
    // A warning nobody reads is the same bug one layer up.
    mockCreateTrigger.mockResolvedValue({
      success: true,
      warnings: ['ReleaseFinnished has never been emitted in this workspace.'],
    });

    const ok = await submitTrigger({ ...base, on: [{ event_type: 'ReleaseFinnished' }] });

    expect(ok).toBe(true);
    expect(toasts.value).toHaveLength(1);
    expect(toasts.value[0].type).toBe('warning');
    expect(toasts.value[0].message).toContain('ReleaseFinnished');
  });

  it('toasts both families on update, the schedule one and the event one', async () => {
    mockUpdateTrigger.mockResolvedValue({
      success: true,
      cron_preview: { next_runs: [], warnings: ['Day-of-month and day-of-week are OR-ed.'] },
      warnings: ['OrderShiped has never been emitted in this workspace.'],
    });

    await submitTrigger({ ...base, triggerId: 't1', on: [{ event_type: 'OrderShiped' }] });

    // Toasts prepend, so the column reads newest first and the cron one is
    // last.
    expect(toasts.value.map(t => t.type)).toEqual(['warning', 'warning']);
    expect(toasts.value[0].message).toContain('OrderShiped');
    expect(toasts.value[1].message).toContain('Day-of-month');
  });

  it('stays quiet when the engine sends no warnings', async () => {
    mockCreateTrigger.mockResolvedValue({ success: true });

    await submitTrigger({ ...base, on: [{ event_type: 'ChangeProposed' }] });

    expect(toasts.value).toHaveLength(0);
  });
});

describe('navigateToTrigger pane reveal', () => {
  beforeEach(() => {
    mockRevealContentPane.mockClear();
    triggers.value = { status: 'loaded', data: [makeTrigger({ id: 't1', name: 'X' })] };
    historicalTriggers.value = { status: 'loaded', data: [] };
    panelOverlay.value = null;
  });

  it('reveals the content pane via the canonical helper (mobile swipe + desktop expand)', async () => {
    // The earlier implementation reached for navigateToPane('content') under
    // an isMobile() gate to compensate for setActiveMenu's bug-prone pane
    // conditional. After consolidating on revealContentPane(), the deep-link
    // path hits one canonical helper that handles BOTH mobile swipe and
    // desktop split-collapsed expansion. Without that, opening a trigger
    // deep-link on a desktop with the split collapsed silently looked like
    // nothing happened.
    await navigateToTrigger('t1');
    expect(mockRevealContentPane).toHaveBeenCalledTimes(1);
  });
});

describe('navigateToTrigger stale-cache reconfirm', () => {
  beforeEach(() => {
    mockRevealContentPane.mockClear();
    mockListTriggers.mockReset();
    toasts.value = [];
    historicalTriggers.value = { status: 'loaded', data: [] };
    panelOverlay.value = null;
  });

  it('re-fetches the source of truth on a cache miss before navigating (sibling just created the trigger)', async () => {
    // The cached list is loaded but momentarily stale — a sibling thread just
    // created `t-new` and the Trigger* SSE refresh hasn't landed. navigateToTrigger
    // must re-fetch (not conclude "no longer exists") and then navigate.
    triggers.value = { status: 'loaded', data: [] };
    mockListTriggers.mockResolvedValue({ triggers: [makeTrigger({ id: 't-new', name: 'Fresh' })] });

    await navigateToTrigger('t-new');

    expect(mockListTriggers).toHaveBeenCalledTimes(1);
    expect(mockRevealContentPane).toHaveBeenCalledTimes(1);
    expect(toasts.value).toHaveLength(0);
  });

  it('toasts a named "no longer exists" error only after a re-fetch still misses', async () => {
    triggers.value = { status: 'loaded', data: [] };
    mockListTriggers.mockResolvedValue({ triggers: [] });

    await navigateToTrigger('gone-id', 'thread "X"');

    expect(mockListTriggers).toHaveBeenCalledTimes(1);
    expect(mockRevealContentPane).not.toHaveBeenCalled();
    expect(toasts.value).toHaveLength(1);
    expect(toasts.value[0].type).toBe('error');
    // Names the id AND where the navigate came from — never a bare generic.
    expect(toasts.value[0].message).toContain('gone-id');
    expect(toasts.value[0].message).toContain('thread "X"');
  });
});

describe('navigateToTrigger lands on the row, not the edit form', () => {
  beforeEach(() => {
    mockRevealContentPane.mockClear();
    mockSetActiveMenu.mockClear();
    mockListTriggers.mockReset();
    toasts.value = [];
    triggers.value = { status: 'loaded', data: [makeTrigger({ id: 't1', name: 'X' })] };
    historicalTriggers.value = { status: 'loaded', data: [] };
    panelOverlay.value = null;
    triggerScrollTarget.value = null;
  });

  it('switches to the Triggers panel with NO overlay and stamps the scroll target', async () => {
    // The row carries Run once, the pause toggle and the last-run chip, which
    // is what "here is your trigger" points at. The edit form carries none of
    // them, and it is one tap away from the row.
    await navigateToTrigger('t1');

    expect(mockSetActiveMenu).toHaveBeenCalledWith('triggers');
    expect(triggerScrollTarget.value).toBe('t1');
  });

  it('closes an edit form left open on another trigger', async () => {
    panelOverlay.value = { type: 'form', form: { type: 'trigger', triggerId: 'other' } };

    await navigateToTrigger('t1');

    // setActiveMenu is called with no overlay argument, so its `null` default
    // clears whatever was open. Pinned because passing the form back would
    // silently restore the old landing.
    expect(mockSetActiveMenu).toHaveBeenCalledWith('triggers');
    expect(mockSetActiveMenu.mock.calls[0]).toHaveLength(1);
  });

  it('stamps no scroll target when the trigger is genuinely gone', async () => {
    triggers.value = { status: 'loaded', data: [] };
    mockListTriggers.mockResolvedValue({ triggers: [] });

    await navigateToTrigger('gone-id');

    expect(triggerScrollTarget.value).toBeNull();
    expect(mockSetActiveMenu).not.toHaveBeenCalled();
  });
});
