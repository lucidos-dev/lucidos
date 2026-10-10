import { describe, it, expect, vi, beforeEach } from 'vitest';

const showToast = vi.fn();
vi.mock('../store', () => ({ showToast }));

const { panelPullTravel, panelRefreshAvailable, panelRefreshing, panelRefreshSucceeded, registerPanelRefresh, runPanelRefresh, showPullTravel } = await import('../panelRefresh');

/** A promise the test settles by hand, to hold a refresh open. */
function deferred() {
  let resolve!: () => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<void>((res, rej) => { resolve = res; reject = rej; });
  return { promise, resolve, reject };
}

describe('runPanelRefresh', () => {
  let cleanups: Array<() => void> = [];
  const register = (noun: string, action: () => Promise<unknown>) => {
    cleanups.push(registerPanelRefresh(noun, action));
  };

  beforeEach(() => {
    for (const c of cleanups) c();
    cleanups = [];
    showToast.mockClear();
  });

  it('reports nothing to refresh until a panel registers', () => {
    expect(panelRefreshAvailable.value).toBe(false);
    register('disk usage', async () => {});
    expect(panelRefreshAvailable.value).toBe(true);
  });

  it('marks the refresh running until the action settles', async () => {
    const d = deferred();
    register('disk usage', () => d.promise);
    const run = runPanelRefresh();
    expect(panelRefreshing.value).toBe(true);
    d.resolve();
    await run;
    expect(panelRefreshing.value).toBe(false);
  });

  it('ignores a second call while one runs, so one pull reads once', async () => {
    const d = deferred();
    const action = vi.fn(() => d.promise);
    register('disk usage', action);
    const first = runPanelRefresh();
    await runPanelRefresh();
    d.resolve();
    await first;
    expect(action).toHaveBeenCalledTimes(1);
  });

  it('counts a refresh whose every read landed as a success', async () => {
    register('disk usage', async () => {});
    const before = panelRefreshSucceeded.value;
    await runPanelRefresh();
    expect(panelRefreshSucceeded.value).toBe(before + 1);
  });

  it('never counts a refresh with a failed read as a success', async () => {
    register('disk usage', async () => {});
    register('plugins', () => Promise.reject(new Error('engine unreachable')));
    const before = panelRefreshSucceeded.value;
    await runPanelRefresh();
    expect(panelRefreshSucceeded.value).toBe(before);
  });

  it('shows a failure, naming what failed', async () => {
    register('disk usage', () => Promise.reject(new Error('engine unreachable')));
    await runPanelRefresh();
    expect(showToast).toHaveBeenCalledWith("Couldn't refresh disk usage: engine unreachable", 'error');
    expect(panelRefreshing.value).toBe(false);
  });

  it('runs every piece the panel registered, and drops one that unmounts', async () => {
    const editorA = vi.fn(async () => {});
    const editorB = vi.fn(async () => {});
    register('agent commands', editorA);
    const unregisterB = registerPanelRefresh('coding-agent tools', editorB);
    await runPanelRefresh();
    expect(editorA).toHaveBeenCalledTimes(1);
    expect(editorB).toHaveBeenCalledTimes(1);
    unregisterB();
    await runPanelRefresh();
    expect(editorA).toHaveBeenCalledTimes(2);
    expect(editorB).toHaveBeenCalledTimes(1);
  });

  it('waits for every piece, and names each one that failed', async () => {
    const slow = deferred();
    register('agent commands', () => slow.promise);
    register('coding-agent tools', () => Promise.reject(new Error('timed out')));
    const run = runPanelRefresh();
    await Promise.resolve();
    expect(panelRefreshing.value).toBe(true);
    slow.resolve();
    await run;
    expect(panelRefreshing.value).toBe(false);
    expect(showToast).toHaveBeenCalledTimes(1);
    expect(showToast).toHaveBeenCalledWith("Couldn't refresh coding-agent tools: timed out", 'error');
  });

  it('settles a piece that unmounts mid-read, so the next refresh can run', async () => {
    const neverLands = new Promise<void>(() => {});
    const unregister = registerPanelRefresh('coding agent binaries', () => neverLands);
    const run = runPanelRefresh();
    expect(panelRefreshing.value).toBe(true);
    unregister();
    await run;
    expect(panelRefreshing.value).toBe(false);
  });

  it('never counts a read cut short by an unmount as a success', async () => {
    const neverLands = new Promise<void>(() => {});
    const unregister = registerPanelRefresh('coding agent binaries', () => neverLands);
    const before = panelRefreshSucceeded.value;
    const run = runPanelRefresh();
    unregister();
    await run;
    expect(panelRefreshSucceeded.value).toBe(before);
  });

  it('does nothing when no panel registered', async () => {
    await runPanelRefresh();
    expect(panelRefreshing.value).toBe(false);
    expect(showToast).not.toHaveBeenCalled();
  });
});

describe('showPullTravel', () => {
  it('keeps the affordance at rest on a panel with nothing to refresh', () => {
    showPullTravel(40);
    expect(panelPullTravel.value).toBe(0);
  });

  it('follows the pull on a refreshable panel', () => {
    const unregister = registerPanelRefresh('disk usage', async () => {});
    showPullTravel(40);
    expect(panelPullTravel.value).toBe(40);
    showPullTravel(0);
    unregister();
  });

  it('stays at rest while a refresh runs, since the header shows it', async () => {
    let finish!: () => void;
    const unregister = registerPanelRefresh('disk usage', () => new Promise<void>((res) => { finish = res; }));
    const run = runPanelRefresh();
    showPullTravel(40);
    expect(panelPullTravel.value).toBe(0);
    finish();
    await run;
    unregister();
  });
});
