// @vitest-environment jsdom
import { describe, it, expect, afterEach, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { ContentRefreshButton } from '../RefreshIndicator';
import { panelRefreshing } from '../../../store/panelRefresh';

vi.mock('../../../store/panelRefresh', async () => {
  const { signal } = await import('@preact/signals');
  return {
    panelRefreshAvailable: signal(true),
    panelRefreshing: signal(false),
    panelRefreshSucceeded: signal(0),
    runPanelRefresh: vi.fn(async () => {}),
  };
});

describe('the content Refresh while it is busy', () => {
  let host: HTMLDivElement | null = null;

  afterEach(() => {
    if (host) { render(null, host); host.remove(); host = null; }
    panelRefreshing.value = false;
  });

  async function mount() {
    host = document.createElement('div');
    document.body.appendChild(host);
    await act(async () => { render(<ContentRefreshButton />, host!); });
    return host.querySelector<HTMLButtonElement>('.content-refresh-btn')!;
  }

  it('stays focusable and keeps its tooltip, marked aria-disabled rather than disabled', async () => {
    panelRefreshing.value = true;
    const button = await mount();
    await act(async () => {});
    expect(button.getAttribute('aria-disabled')).toBe('true');
    expect(button.disabled).toBe(false);
    expect(button.getAttribute('data-tooltip')).toBeTruthy();
  });

  it('is open to a press again once the refresh has returned', async () => {
    const button = await mount();
    expect(button.getAttribute('aria-disabled')).toBe('false');
    expect(button.disabled).toBe(false);
  });
});
