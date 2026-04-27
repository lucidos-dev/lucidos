/**
 * Tests for CC model/effort per-thread behavior.
 *
 * Model and reasoning effort are per-thread — they come from backend events
 * (CodingAgentSettingsChanged) or the active session, never from cross-thread cache.
 * New threads start with null (no label shown = defaults).
 */
import { describe, it, expect } from 'vitest';

interface CCCommandsResponse {
  control_commands: unknown[];
  builtin_commands: string[];
  skill_commands: string[];
  current_model: string | null;
  current_reasoning_effort: string | null;
  has_active_session: boolean;
}

// ---------------------------------------------------------------------------
// Mirror the loadCommands logic from CCControlMenu.tsx
// ---------------------------------------------------------------------------

interface State {
  pendingEffort: string | null;
  pendingModel: string | null;
  currentEffort: string | null;
  currentModel: string | null;
  hasActiveSession: boolean;
}

/** Simulate loadCommands() — always updates model/effort from backend response.
 *  Clears pending when active session confirms. No cross-thread caching. */
function handleLoadCommands(state: State, res: CCCommandsResponse): State {
  const next = { ...state };
  next.hasActiveSession = res.has_active_session;
  if (res.has_active_session) {
    next.pendingEffort = null;
    next.pendingModel = null;
  }
  next.currentEffort = (res.current_reasoning_effort as string) ?? null;
  next.currentModel = (res.current_model as string) ?? null;
  return next;
}

/** Mirror currentValueLabel() — pending overrides backend value. */
function currentValueLabel(state: State, field: 'effort' | 'model'): string | null {
  if (field === 'effort') return state.pendingEffort ?? state.currentEffort;
  return state.pendingModel ?? state.currentModel;
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe('CC model/effort: per-thread, no cross-thread cache', () => {
  // Fresh thread — no prior state
  const freshState: State = {
    pendingEffort: null,
    pendingModel: null,
    currentEffort: null,
    currentModel: null,
    hasActiveSession: false,
  };

  it('new thread starts with no model/effort labels', () => {
    expect(currentValueLabel(freshState, 'model')).toBeNull();
    expect(currentValueLabel(freshState, 'effort')).toBeNull();
  });

  it('backend response populates per-thread values', () => {
    const res: CCCommandsResponse = {
      control_commands: [],
      builtin_commands: ['help'],
      skill_commands: [],
      current_model: 'opus[1m]',
      current_reasoning_effort: 'max',
      has_active_session: true,
    };
    const state = handleLoadCommands(freshState, res);
    expect(currentValueLabel(state, 'model')).toBe('opus[1m]');
    expect(currentValueLabel(state, 'effort')).toBe('max');
  });

  it('null response clears stale values (thread switch)', () => {
    // Thread A had values
    const threadA: State = {
      ...freshState,
      currentModel: 'opus[1m]',
      currentEffort: 'max',
    };
    // Switch to thread B — backend returns null (no CC session, no events)
    const res: CCCommandsResponse = {
      control_commands: [],
      builtin_commands: [],
      skill_commands: [],
      current_model: null,
      current_reasoning_effort: null,
      has_active_session: false,
    };
    const state = handleLoadCommands(threadA, res);
    expect(currentValueLabel(state, 'model')).toBeNull();
    expect(currentValueLabel(state, 'effort')).toBeNull();
  });
});

describe('CC pending preferences survive until session confirms', () => {
  const freshState: State = {
    pendingEffort: null,
    pendingModel: null,
    currentEffort: null,
    currentModel: null,
    hasActiveSession: false,
  };

  it('pending values shown before session starts', () => {
    const state: State = { ...freshState, pendingEffort: 'max', pendingModel: 'opus[1m]' };
    expect(currentValueLabel(state, 'effort')).toBe('max');
    expect(currentValueLabel(state, 'model')).toBe('opus[1m]');
  });

  it('pending preserved through cache responses (no active session)', () => {
    let state: State = { ...freshState, pendingEffort: 'max', pendingModel: 'opus[1m]' };
    const cacheRes: CCCommandsResponse = {
      control_commands: [],
      builtin_commands: ['help'],
      skill_commands: [],
      current_model: null,
      current_reasoning_effort: null,
      has_active_session: false,
    };
    state = handleLoadCommands(state, cacheRes);
    state = handleLoadCommands(state, cacheRes);
    expect(currentValueLabel(state, 'effort')).toBe('max');
    expect(currentValueLabel(state, 'model')).toBe('opus[1m]');
  });

  it('pending cleared when session confirms with actual values', () => {
    let state: State = { ...freshState, pendingEffort: 'max', pendingModel: 'opus[1m]' };
    const sessionRes: CCCommandsResponse = {
      control_commands: [],
      builtin_commands: [],
      skill_commands: [],
      current_model: 'opus[1m]',
      current_reasoning_effort: 'max',
      has_active_session: true,
    };
    state = handleLoadCommands(state, sessionRes);
    expect(state.pendingEffort).toBeNull();
    expect(state.pendingModel).toBeNull();
    expect(currentValueLabel(state, 'effort')).toBe('max');
    expect(currentValueLabel(state, 'model')).toBe('opus[1m]');
  });
});

describe('CC model/effort from thread events (no active session)', () => {
  const freshState: State = {
    pendingEffort: null,
    pendingModel: null,
    currentEffort: null,
    currentModel: null,
    hasActiveSession: false,
  };

  it('existing thread with CodingAgentSettingsChanged events shows stored values', () => {
    // Backend returns values from CodingAgentSettingsChanged events for this thread
    const res: CCCommandsResponse = {
      control_commands: [],
      builtin_commands: ['help'],
      skill_commands: [],
      current_model: 'opus',
      current_reasoning_effort: 'high',
      has_active_session: false,
    };
    const state = handleLoadCommands(freshState, res);
    expect(currentValueLabel(state, 'model')).toBe('opus');
    expect(currentValueLabel(state, 'effort')).toBe('high');
  });

  it('values update when backend returns different thread settings', () => {
    // First load — thread A settings
    let state = handleLoadCommands(freshState, {
      control_commands: [],
      builtin_commands: [],
      skill_commands: [],
      current_model: 'opus',
      current_reasoning_effort: 'high',
      has_active_session: false,
    });
    // Switch to thread B — different settings from events
    state = handleLoadCommands(state, {
      control_commands: [],
      builtin_commands: [],
      skill_commands: [],
      current_model: 'sonnet',
      current_reasoning_effort: 'max',
      has_active_session: false,
    });
    expect(currentValueLabel(state, 'model')).toBe('sonnet');
    expect(currentValueLabel(state, 'effort')).toBe('max');
  });
});
