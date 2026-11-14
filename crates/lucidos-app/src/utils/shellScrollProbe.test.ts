// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('./clientLog', () => ({ postClientLog: vi.fn() }));

import { postClientLog } from './clientLog';
import { reportShellSnapBack, resetShellScrollProbe } from './shellScrollProbe';

describe('reportShellSnapBack', () => {
  let now = 0;

  beforeEach(() => {
    vi.useFakeTimers();
    resetShellScrollProbe();
    vi.mocked(postClientLog).mockClear();
    vi.spyOn(performance, 'now').mockImplementation(() => now);
    now = 10_000;
  });

  afterEach(() => {
    resetShellScrollProbe();
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it('logs which box moved, how far, and what held focus', () => {
    const input = document.createElement('input');
    input.className = 'prompt-textarea extra';
    document.body.appendChild(input);
    input.focus();
    reportShellSnapBack({ box: 'app-shell', top: 22, left: 0 });
    expect(postClientLog).toHaveBeenCalledWith('shell-scroll', 'snap-back', expect.objectContaining({
      box: 'app-shell', top: 22, left: 0, suppressed: 0, focus: 'input.prompt-textarea.extra',
    }));
    input.remove();
  });

  it('logs one line a second and counts the rest into the next', () => {
    reportShellSnapBack({ box: 'document', top: 1, left: 0 });
    now += 200;
    reportShellSnapBack({ box: 'document', top: 2, left: 0 });
    reportShellSnapBack({ box: 'document', top: 3, left: 0 });
    expect(postClientLog).toHaveBeenCalledTimes(1);
    now += 1000;
    reportShellSnapBack({ box: 'document', top: 4, left: 0 });
    expect(postClientLog).toHaveBeenCalledTimes(2);
    expect(vi.mocked(postClientLog).mock.calls[1][2]).toMatchObject({ top: 4, suppressed: 2 });
  });

  it('logs the last snap-back of a burst when its second ends', () => {
    reportShellSnapBack({ box: 'document', top: 1, left: 0 });
    now += 200;
    reportShellSnapBack({ box: 'document', top: 2, left: 0 });
    reportShellSnapBack({ box: 'document', top: 3, left: 0 });
    expect(postClientLog).toHaveBeenCalledTimes(1);
    now += 800;
    vi.advanceTimersByTime(800);
    expect(postClientLog).toHaveBeenCalledTimes(2);
    expect(vi.mocked(postClientLog).mock.calls[1][2]).toMatchObject({ top: 3, suppressed: 1 });
    vi.advanceTimersByTime(5000);
    expect(postClientLog).toHaveBeenCalledTimes(2);
  });
});
