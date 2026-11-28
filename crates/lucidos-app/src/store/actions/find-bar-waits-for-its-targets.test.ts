// @vitest-environment jsdom
import { describe, it, expect, vi, beforeAll, beforeEach } from 'vitest';

const chunk = vi.hoisted(() => {
  const state = { release: () => {}, arrived: Promise.resolve() };
  return {
    state,
    cold() {
      state.arrived = new Promise<void>((resolve) => { state.release = resolve; });
    },
  };
});
vi.mock('./find-targets', async () => {
  await chunk.state.arrived;
  return { contentTarget: vi.fn(), threadTarget: vi.fn() };
});

type FindBar = typeof import('./find-bar');

async function coldFindBar(): Promise<FindBar> {
  vi.resetModules();
  chunk.cold();
  return import('./find-bar');
}

async function land(bar: FindBar): Promise<void> {
  chunk.state.release();
  await bar.findTargets.preload();
  await new Promise((r) => setTimeout(r, 0));
}

describe('opening the find bar before its targets arrive', () => {
  beforeAll(async () => { await import('./find-bar'); });
  beforeEach(() => { vi.resetModules(); });

  it('opens once they land, and not before', async () => {
    const bar = await coldFindBar();
    bar.openFind('content', { focus: false });
    expect(bar.findSurface.value).toBeNull();
    await land(bar);
    expect(bar.findSurface.value).toBe('content');
  });

  it('takes a waiting open back on a second toggle', async () => {
    const bar = await coldFindBar();
    bar.toggleFind('content');
    bar.toggleFind('content');
    await land(bar);
    expect(bar.findSurface.value).toBeNull();
  });

  it('takes a waiting open back on a reset', async () => {
    const bar = await coldFindBar();
    bar.openFind('thread', { focus: false });
    bar.resetFind();
    await land(bar);
    expect(bar.findSurface.value).toBeNull();
  });

  it('takes a waiting open back when its view goes away', async () => {
    const bar = await coldFindBar();
    bar.openFind('content', { focus: false });
    bar.endFindOn('content');
    await land(bar);
    expect(bar.findSurface.value).toBeNull();
  });

  it('lets a toggle on the other surface replace a waiting open', async () => {
    const bar = await coldFindBar();
    bar.openFind('thread', { focus: false });
    bar.toggleFind('content');
    await land(bar);
    expect(bar.findSurface.value).toBe('content');
  });

  it('takes a waiting open back on a press anywhere', async () => {
    const bar = await coldFindBar();
    bar.openFind('content', { focus: false });
    document.body.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true }));
    await land(bar);
    expect(bar.findSurface.value).toBeNull();
  });
});
