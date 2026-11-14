import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const refreshThreadEvents = vi.hoisted(() => vi.fn<(id: string) => Promise<boolean>>());
const restartEventStream = vi.hoisted(() => vi.fn<() => 'direct' | 'shared' | null>());
const postClientLog = vi.hoisted(() => vi.fn());
const frames = vi.hoisted(() => ({ received: 0 }));

vi.mock('./thread-loading', () => ({ refreshThreadEvents }));
vi.mock('./event-stream', () => ({ restartEventStream, eventFramesReceived: () => frames.received }));
vi.mock('../../utils/clientLog', () => ({ postClientLog }));

import { SEND_CONFIRM_MS, probeStreamAfterSend } from './send-probe';

/** Let the probe's awaited refetch settle. */
const settle = () => vi.advanceTimersByTimeAsync(0);

/** A refetch that finds each thread's confirming event the stream never
 *  brought. Returns that thread's `confirmed` check. */
function refetchFinds(): (threadId: string) => () => boolean {
  const persisted = new Set<string>();
  refreshThreadEvents.mockImplementation(async (threadId) => {
    persisted.add(threadId);
    return true;
  });
  return (threadId) => () => persisted.has(threadId);
}

/** Each test starts an hour after the last, clear of the restart cooldown. */
let clock = 0;

describe('probeStreamAfterSend', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    clock += 3_600_000;
    vi.setSystemTime(clock);
    frames.received = 0;
    refreshThreadEvents.mockReset();
    restartEventStream.mockReset().mockReturnValue('shared');
    postClientLog.mockReset();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it('does nothing when the confirming frame arrived in time', async () => {
    probeStreamAfterSend('t1', () => true);

    await vi.advanceTimersByTimeAsync(SEND_CONFIRM_MS * 2);

    expect(refreshThreadEvents).not.toHaveBeenCalled();
    expect(restartEventStream).not.toHaveBeenCalled();
  });

  it('waits the full confirm window before probing', async () => {
    refreshThreadEvents.mockResolvedValue(false);
    probeStreamAfterSend('t1', () => false);

    await vi.advanceTimersByTimeAsync(SEND_CONFIRM_MS - 1);
    expect(refreshThreadEvents).not.toHaveBeenCalled();

    await vi.advanceTimersByTimeAsync(1);
    expect(refreshThreadEvents).toHaveBeenCalledWith('t1');
  });

  it('refetches and restarts the stream when the refetch shows the frame it missed', async () => {
    let persisted = false;
    refreshThreadEvents.mockImplementation(async () => {
      persisted = true;
      return true;
    });
    probeStreamAfterSend('t1', () => persisted);

    await vi.advanceTimersByTimeAsync(SEND_CONFIRM_MS);
    await settle();

    expect(refreshThreadEvents).toHaveBeenCalledTimes(1);
    expect(restartEventStream).toHaveBeenCalledTimes(1);
    expect(postClientLog).toHaveBeenCalledWith('event-stream', 'missed-frame', {
      transport: 'shared',
      waitedMs: SEND_CONFIRM_MS,
    });
  });

  it('never restarts after a refetch that did not land', async () => {
    refreshThreadEvents.mockResolvedValue(false);
    probeStreamAfterSend('t1', () => false);

    await vi.advanceTimersByTimeAsync(SEND_CONFIRM_MS);
    await settle();

    expect(restartEventStream).not.toHaveBeenCalled();
    expect(postClientLog).not.toHaveBeenCalled();
  });

  it('never restarts after a refetch that still shows no confirming event', async () => {
    refreshThreadEvents.mockResolvedValue(true);
    probeStreamAfterSend('t1', () => false);

    await vi.advanceTimersByTimeAsync(SEND_CONFIRM_MS);
    await settle();

    expect(restartEventStream).not.toHaveBeenCalled();
    expect(postClientLog).not.toHaveBeenCalled();
  });

  it('never restarts when the stream delivered a frame during the refetch', async () => {
    let persisted = false;
    refreshThreadEvents.mockImplementation(async () => {
      frames.received += 1;
      persisted = true;
      return true;
    });
    probeStreamAfterSend('t1', () => persisted);

    await vi.advanceTimersByTimeAsync(SEND_CONFIRM_MS);
    await settle();

    expect(restartEventStream).not.toHaveBeenCalled();
    expect(postClientLog).not.toHaveBeenCalled();
  });

  it('restarts once for a burst of sends over one stalled stream', async () => {
    const confirmed = refetchFinds();
    probeStreamAfterSend('t1', confirmed('t1'));
    probeStreamAfterSend('t2', confirmed('t2'));
    await vi.advanceTimersByTimeAsync(1000);
    probeStreamAfterSend('t3', confirmed('t3'));

    await vi.advanceTimersByTimeAsync(SEND_CONFIRM_MS);
    await settle();

    expect(refreshThreadEvents).toHaveBeenCalledTimes(3);
    expect(restartEventStream).toHaveBeenCalledTimes(1);
  });

  it('restarts again once the cooldown has passed', async () => {
    const confirmed = refetchFinds();
    probeStreamAfterSend('t1', confirmed('t1'));
    await vi.advanceTimersByTimeAsync(SEND_CONFIRM_MS);
    await settle();

    probeStreamAfterSend('t2', confirmed('t2'));
    await vi.advanceTimersByTimeAsync(SEND_CONFIRM_MS);
    await settle();

    expect(restartEventStream).toHaveBeenCalledTimes(2);
  });

  it('logs nothing when no stream was up to restart', async () => {
    restartEventStream.mockReturnValue(null);
    let persisted = false;
    refreshThreadEvents.mockImplementation(async () => {
      persisted = true;
      return true;
    });
    probeStreamAfterSend('t1', () => persisted);

    await vi.advanceTimersByTimeAsync(SEND_CONFIRM_MS);
    await settle();

    expect(postClientLog).not.toHaveBeenCalled();
  });
});
