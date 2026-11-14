import { describe, it, expect } from 'vitest';
import { singleFlight } from './singleFlight';

/** An ask that waits until the test lets it finish. */
function gatedAsks() {
  const releases: (() => void)[] = [];
  const ask = () => new Promise<void>((resolve) => { releases.push(resolve); });
  return { ask, releases };
}

describe('singleFlight', () => {
  it('lets a plain call join the ask in flight', async () => {
    const { ask, releases } = gatedAsks();
    const run = singleFlight(ask);
    const first = run();
    const joined = run();
    expect(releases).toHaveLength(1);
    releases[0]();
    await Promise.all([first, joined]);
    expect(releases).toHaveLength(1);
  });

  /** A gateway added while a poll is in flight: the poll began before it was
   *  saved, so only a second ask can list its workspaces. */
  it('runs one more ask for a fresh call, and waits for it', async () => {
    const { ask, releases } = gatedAsks();
    const run = singleFlight(ask);
    void run();
    let freshDone = false;
    const fresh = run(true).then(() => { freshDone = true; });
    releases[0]();
    await new Promise((r) => setTimeout(r, 0));
    expect(releases).toHaveLength(2);
    expect(freshDone).toBe(false);
    releases[1]();
    await fresh;
    expect(freshDone).toBe(true);
  });

  it('starts a new ask once the last one has ended', async () => {
    const { ask, releases } = gatedAsks();
    const run = singleFlight(ask);
    const first = run();
    releases[0]();
    await first;
    void run();
    expect(releases).toHaveLength(2);
  });

  it('frees the slot when an ask fails', async () => {
    const run = singleFlight(() => Promise.reject(new Error('down')));
    await expect(run()).rejects.toThrow('down');
    await expect(run()).rejects.toThrow('down');
  });
});
