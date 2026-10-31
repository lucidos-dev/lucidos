// @vitest-environment jsdom
/**
 * The Memory page's inspector lists Classic's saved memories, so a Tree
 * workspace shows only the Tree section and its summary tree browser. Until
 * preferences load the inspector stays, so a Classic workspace never sees it
 * arrive late.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

vi.mock('../MemoryInspector', async () => {
  const { h } = await import('preact');
  return { MemoryInspector: () => h('div', { 'data-role': 'memory-inspector' }) };
});
vi.mock('../SummaryTreeBrowser', async () => {
  const { h } = await import('preact');
  return { SummaryTreeBrowser: () => h('div', { 'data-role': 'summary-tree-browser' }) };
});
vi.mock('../MemoryModuleSection', async () => {
  const { h } = await import('preact');
  return { MemoryModuleSection: () => h('div', { 'data-role': 'memory-module' }) };
});

async function mount(host: HTMLElement, prefs: 'not-loaded' | 'classic' | 'tree'): Promise<() => void> {
  const { h, render } = await import('preact');
  const store = await import('../../../store/store');
  store.preferences.value = prefs === 'not-loaded'
    ? { status: 'not-loaded' }
    : { status: 'loaded', data: { memory_module: prefs } };
  const { MemoryPage } = await import('../MemoryPage');
  render(h(MemoryPage, {}), host);
  return () => render(null, host);
}

describe('the Memory page', () => {
  let host: HTMLElement;
  let unmount: () => void = () => {};

  beforeEach(() => {
    host = document.createElement('div');
    document.body.appendChild(host);
  });

  afterEach(() => {
    unmount();
    host.remove();
  });

  it('shows the inspector on Classic', async () => {
    unmount = await mount(host, 'classic');
    expect(host.querySelector('[data-role="memory-module"]')).not.toBeNull();
    expect(host.querySelector('[data-role="memory-inspector"]')).not.toBeNull();
    expect(host.querySelector('[data-role="summary-tree-browser"]')).toBeNull();
  });

  it('keeps the inspector while preferences load', async () => {
    unmount = await mount(host, 'not-loaded');
    expect(host.querySelector('[data-role="memory-inspector"]')).not.toBeNull();
    expect(host.querySelector('[data-role="summary-tree-browser"]')).toBeNull();
  });

  it('shows only the Tree section on Tree', async () => {
    unmount = await mount(host, 'tree');
    expect(host.querySelector('[data-role="memory-module"]')).not.toBeNull();
    expect(host.querySelector('[data-role="memory-inspector"]')).toBeNull();
    expect(host.querySelector('[data-role="summary-tree-browser"]')).not.toBeNull();
  });
});
