// @vitest-environment jsdom
/**
 * The three semantics a settings page depends on, pinned away from any page.
 * Never on mount, once per move, and a pause that defers rather than drops.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import type { Mock } from 'vitest';
import { render } from 'preact';
import { useVersionedRefresh } from '../useVersionedRefresh';

function Probe({ version, paused, onChange }: {
  version: number;
  paused: boolean;
  onChange: () => void;
}) {
  useVersionedRefresh(version, paused, onChange);
  return null;
}

/** Preact runs an effect after the next animation frame, which jsdom ticks on
 *  a ~16ms timer, and a loaded parallel run stretches those frames. So the
 *  positive assertions poll and the negative ones wait a fixed, generous
 *  window. A fixed 20ms for both flaked under the full suite. */
async function waitForCalls(fn: Mock<() => void>, n: number): Promise<void> {
  for (let waited = 0; waited < 1000 && fn.mock.calls.length < n; waited += 10) {
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
}

async function settle(): Promise<void> {
  await new Promise((resolve) => setTimeout(resolve, 200));
}

describe('useVersionedRefresh', () => {
  let host: HTMLElement;
  let onChange: Mock<() => void>;

  beforeEach(() => {
    host = document.createElement('div');
    document.body.appendChild(host);
    onChange = vi.fn<() => void>();
  });

  afterEach(() => {
    render(null, host);
    host.remove();
  });

  it('does not fire on mount, however high the version already is', async () => {
    render(<Probe version={7} paused={false} onChange={onChange} />, host);
    await settle();
    expect(onChange).not.toHaveBeenCalled();
  });

  it('fires once per move', async () => {
    render(<Probe version={0} paused={false} onChange={onChange} />, host);
    render(<Probe version={1} paused={false} onChange={onChange} />, host);
    await waitForCalls(onChange, 1);
    expect(onChange).toHaveBeenCalledTimes(1);

    render(<Probe version={2} paused={false} onChange={onChange} />, host);
    await waitForCalls(onChange, 2);
    expect(onChange).toHaveBeenCalledTimes(2);
  });

  it('does not fire on a re-render that leaves the version alone', async () => {
    render(<Probe version={3} paused={false} onChange={onChange} />, host);
    render(<Probe version={3} paused={false} onChange={onChange} />, host);
    await settle();
    expect(onChange).not.toHaveBeenCalled();
  });

  it('defers while paused, then fires when the pause clears', async () => {
    render(<Probe version={0} paused={true} onChange={onChange} />, host);
    render(<Probe version={1} paused={true} onChange={onChange} />, host);
    await settle();
    expect(onChange).not.toHaveBeenCalled();

    // The frame that arrived during the write is owed, not lost.
    render(<Probe version={1} paused={false} onChange={onChange} />, host);
    await waitForCalls(onChange, 1);
    expect(onChange).toHaveBeenCalledTimes(1);
  });

  it('coalesces several moves made under one pause into one call', async () => {
    render(<Probe version={0} paused={true} onChange={onChange} />, host);
    render(<Probe version={1} paused={true} onChange={onChange} />, host);
    render(<Probe version={2} paused={true} onChange={onChange} />, host);
    render(<Probe version={3} paused={false} onChange={onChange} />, host);
    await waitForCalls(onChange, 1);
    expect(onChange).toHaveBeenCalledTimes(1);
  });

  it('calls the latest closure, not the one the version moved under', async () => {
    const first: Mock<() => void> = vi.fn();
    const second: Mock<() => void> = vi.fn();
    render(<Probe version={0} paused={false} onChange={first} />, host);
    render(<Probe version={1} paused={false} onChange={second} />, host);
    await waitForCalls(second, 1);
    expect(first).not.toHaveBeenCalled();
    expect(second).toHaveBeenCalledTimes(1);
  });
});
