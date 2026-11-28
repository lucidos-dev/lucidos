// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { SETTINGS_ANCHOR_WAIT_MS, SettingsView } from '../SettingsView';
import { providerAnchor } from '../../../store/modelProviders';
import {
  credentials,
  preferences,
  settingsLinkedFragment,
  settingsScrollTarget,
  settingsSubview,
  toasts,
} from '../../../store/store';

describe('SettingsView fragment landing', () => {
  let host: HTMLDivElement;
  let scrolled: HTMLElement[];

  beforeEach(() => {
    vi.useFakeTimers();
    scrolled = [];
    window.HTMLElement.prototype.scrollIntoView = vi.fn(function (this: HTMLElement) {
      scrolled.push(this);
    });
    host = document.createElement('div');
    document.body.appendChild(host);
    preferences.value = { status: 'loaded', data: {} };
    credentials.value = { status: 'loaded', data: [] };
    settingsSubview.value = 'providers';
    toasts.value = [];
  });

  afterEach(() => {
    render(null, host);
    host.remove();
    settingsSubview.value = 'main';
    settingsScrollTarget.value = null;
    settingsLinkedFragment.value = null;
    preferences.value = { status: 'not-loaded' };
    credentials.value = { status: 'not-loaded' };
    toasts.value = [];
    vi.useRealTimers();
  });

  async function mount() {
    await act(() => {
      render(<SettingsView />, host);
    });
  }

  async function link(fragment: string) {
    await mount();
    await act(() => {
      settingsLinkedFragment.value = fragment;
    });
  }

  async function waitOut() {
    await act(() => {
      vi.advanceTimersByTime(SETTINGS_ANCHOR_WAIT_MS);
    });
  }

  it('scrolls to the row a fragment names, with no toast', async () => {
    await link('xai');
    expect(scrolled.map((el) => el.dataset.searchAnchor)).toEqual([providerAnchor('xai')]);
    await waitOut();
    expect(toasts.value).toEqual([]);
    expect(settingsLinkedFragment.value).toBeNull();
  });

  it('scrolls to and marks the colon-anchored sandbox row without a toast', async () => {
    settingsSubview.value = 'permissions';
    await link('agent-sandbox:unsandboxed');
    const row = host.querySelector<HTMLElement>('[data-search-anchor="agent-sandbox:unsandboxed"]')!;
    expect(row).not.toBeNull();
    expect(scrolled).toEqual([row]);
    expect(row.classList.contains('nav-focus-stuck')).toBe(true);
    await waitOut();
    expect(toasts.value).toEqual([]);
  });

  it('lands a retired full anchor on its current row', async () => {
    await link('models:vertex-ai');
    expect(scrolled.map(el => el.dataset.searchAnchor)).toEqual([providerAnchor('vertex')]);
    await waitOut();
    expect(toasts.value).toEqual([]);
  });

  it('lands a row inside a fold on the block its search entry names', async () => {
    await link('vertex-region');
    expect(scrolled.map((el) => el.dataset.searchAnchor)).toEqual([providerAnchor('vertex')]);
  });

  it('lands a retired row inside a fold on its current ancestor', async () => {
    await link('models:region');
    expect(scrolled.map(el => el.dataset.searchAnchor)).toEqual([providerAnchor('vertex')]);
    await waitOut();
    expect(toasts.value).toEqual([]);
  });

  it('waits for a row that renders after the page', async () => {
    await link('late-row');
    expect(scrolled).toEqual([]);
    const late = document.createElement('div');
    late.dataset.searchAnchor = 'providers:late-row';
    await act(() => {
      host.appendChild(late);
    });
    await vi.waitFor(() => expect(scrolled).toEqual([late]));
    await waitOut();
    expect(toasts.value).toEqual([]);
  });

  it('reports a fragment that names no row, and names the page', async () => {
    await link('no-such-row');
    expect(toasts.value).toEqual([]);
    await waitOut();
    expect(scrolled).toEqual([]);
    expect(toasts.value.map((t) => t.message)).toEqual(['No "no-such-row" on Providers settings']);
  });

  it('stays silent for a missed anchor no link named', async () => {
    await mount();
    await act(() => {
      settingsScrollTarget.value = 'providers:no-such-row';
    });
    await waitOut();
    expect(toasts.value).toEqual([]);
    expect(settingsScrollTarget.value).toBeNull();
  });
});
