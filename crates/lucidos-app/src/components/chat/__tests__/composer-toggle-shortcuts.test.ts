import { describe, it, expect, beforeEach, vi } from 'vitest';

vi.mock('../../../store/voice', () => ({ pressCallToggle: vi.fn() }));

import { pressCallToggleIfShown } from '../PromptRowControls';
import { pressCallToggle } from '../../../store/voice';
import { focusedThreadId, preferences, threadMap, toasts } from '../../../store/store';
import { makeThreadState } from '../../../store/actions/threads-test-helpers';

function focus(thread: ReturnType<typeof makeThreadState> | null): void {
  threadMap.value = thread ? new Map([[thread.meta.id, thread]]) : new Map();
  focusedThreadId.value = thread?.meta.id ?? null;
}

beforeEach(() => {
  vi.clearAllMocks();
  toasts.value = [];
  preferences.value = { status: 'loaded', data: { voice_enabled: 'true' } };
});

describe('the call shortcut', () => {
  it('presses the call toggle on the home thread', () => {
    focus(makeThreadState('t1', { meta: { home: true } }));
    pressCallToggleIfShown();
    expect(pressCallToggle).toHaveBeenCalledTimes(1);
  });

  it('points to the home thread from another Lucidos Agent thread', () => {
    focus(makeThreadState('t1'));
    pressCallToggleIfShown();
    expect(pressCallToggle).not.toHaveBeenCalled();
    expect(toasts.value.slice(-1)[0]?.message).toBe('Open your home thread with the Lucidos Agent to start a call.');
  });

  it('points to the Lucidos Agent from a coding-agent thread', () => {
    focus(makeThreadState('t1', { meta: { channel: 'claude_code', home: true } }));
    pressCallToggleIfShown();
    expect(pressCallToggle).not.toHaveBeenCalled();
    expect(toasts.value.slice(-1)[0]?.message).toBe('Open your home thread with the Lucidos Agent to start a call.');
  });

  it('points to voice settings while voice is off', () => {
    preferences.value = { status: 'loaded', data: {} };
    focus(makeThreadState('t1', { meta: { home: true } }));
    pressCallToggleIfShown();
    expect(pressCallToggle).not.toHaveBeenCalled();
    expect(toasts.value.slice(-1)[0]?.action?.label).toBe('Settings → Models → Voice');
  });
});
