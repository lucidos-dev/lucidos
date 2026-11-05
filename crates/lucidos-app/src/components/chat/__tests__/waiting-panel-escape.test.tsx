// @vitest-environment jsdom
/**
 * One Escape undoes one step. In the waiting panel's condition drill-in, the
 * first Escape goes back to the list and the second closes the panel. That is
 * how `ModelSelectionPicker`'s steps answer Escape through the overlay stack.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

import { WaitingPanelHost, closeWaitingPanel, waitingIndicatorAction, waitingPanelCondition } from '../WaitingPanel';
import { focusedThreadId, threadMap } from '../../../store/store';
import { _resetOverlayStackForTesting, dismissTopOverlay } from '../../../store/overlayStack';
import type { EventWaitSummary, ThreadMeta, ThreadState } from '../../../store/thread-events';

vi.mock('../../../store/actions/threads', () => ({ focusThreadOrBootstrap: () => {} }));

function settled(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 0));
}

const WAIT = {
  wait_id: 'w1',
  reason: 'Waiting for a review',
  on: [{ event_type: 'PullRequestReviewed', condition: { pr: 412 } }],
  expires_at: new Date(Date.now() + 3_600_000).toISOString(),
} as EventWaitSummary;

function waitingThread(): ThreadState {
  return {
    meta: {
      id: 't1',
      title: 'Thread',
      status: 'idle',
      liveEventWaitCount: 1,
      liveEventWaits: [WAIT],
      activeChildrenCount: 0,
      waitingChildrenCount: 0,
    } as unknown as ThreadMeta,
    events: new Map(),
  } as unknown as ThreadState;
}

describe('the waiting panel drill-in answers Escape one step at a time', () => {
  let host: HTMLDivElement;
  let anchor: HTMLButtonElement;

  beforeEach(() => {
    _resetOverlayStackForTesting();
    threadMap.value = new Map([['t1', waitingThread()]]);
    focusedThreadId.value = 't1';
    anchor = document.createElement('button');
    document.body.appendChild(anchor);
    host = document.createElement('div');
    document.body.appendChild(host);
    render(<WaitingPanelHost />, host);
  });

  afterEach(() => {
    closeWaitingPanel();
    render(null, host);
    host.remove();
    anchor.remove();
    focusedThreadId.value = null;
    threadMap.value = new Map();
  });

  const panel = () => document.querySelector('[data-role="waiting-panel"]');

  it('steps back to the list first, then closes', async () => {
    waitingIndicatorAction()!.onMenuClick!(anchor);
    await settled();
    expect(panel()).not.toBeNull();

    waitingPanelCondition.value = { eventType: 'PullRequestReviewed', conditions: [{ pr: 412 }] };
    await settled();
    expect(document.querySelector('[data-role="waiting-condition"]')).not.toBeNull();
    // The drill-in replaced the focused door, so focus moved to the way back.
    expect(document.activeElement?.getAttribute('data-role')).toBe('surface-back');

    dismissTopOverlay();
    await settled();
    expect(waitingPanelCondition.value).toBeNull();
    expect(panel(), 'the first Escape keeps the panel open').not.toBeNull();
    expect(document.querySelector('.event-wait-list')).not.toBeNull();
    // Coming back out, focus lands on the door that leads in again.
    expect(document.activeElement?.getAttribute('data-role')).toBe('event-wait-condition');

    dismissTopOverlay();
    await settled();
    expect(panel(), 'the second Escape closes it').toBeNull();
  });

  it('falls back to the list when the drilled-in wait is gone', async () => {
    waitingIndicatorAction()!.onMenuClick!(anchor);
    await settled();
    // The fallback is a `useEffect`, which Preact defers past the next frame,
    // so a bare timer can win the race. `act` flushes it.
    await act(async () => {
      waitingPanelCondition.value = { eventType: 'DeployFinished', conditions: [{ env: 'prod' }] };
    });
    expect(waitingPanelCondition.value, 'no live wait watches DeployFinished').toBeNull();
    expect(document.querySelector('.event-wait-list')).not.toBeNull();
  });

  it('opens on the list, never on a condition left over from last time', async () => {
    waitingPanelCondition.value = { eventType: 'PullRequestReviewed', conditions: [{ pr: 412 }] };
    waitingIndicatorAction()!.onMenuClick!(anchor);
    await settled();
    expect(waitingPanelCondition.value).toBeNull();
    expect(document.querySelector('.event-wait-list')).not.toBeNull();
  });
});
