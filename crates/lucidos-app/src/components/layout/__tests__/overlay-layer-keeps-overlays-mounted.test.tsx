// @vitest-environment jsdom
/**
 * Entering or leaving an app's native fullscreen moves the host's overlays into
 * and out of the fullscreen panel. It must MOVE them, never remount them: a
 * remount resets every overlay's own state, so Search Everywhere lost its query
 * and a prompt lost what the reader had typed (ADR 0437).
 */
import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { render } from 'preact';
import { useState } from 'preact/hooks';
import { act } from 'preact/test-utils';

import { OverlayLayer } from '../OverlayLayer';
import { UiBlockingOverlay } from '../UiBlockingOverlay';
import { appFullscreenHost } from '../../../store/appFullscreenHost';
import { clientRefreshing } from '../../../hooks/sw-update';

/** An overlay with state of its own, as Search Everywhere keeps its query. */
function StatefulOverlay() {
  const [query, setQuery] = useState('');
  return (
    <input class="probe" value={query}
      onInput={(e) => setQuery((e.currentTarget as HTMLInputElement).value)} />
  );
}

function Toast() {
  return <div class="toast-container" />;
}

const probe = () => document.querySelector<HTMLInputElement>('.probe')!;

async function type(text: string) {
  await act(() => {
    probe().value = text;
    probe().dispatchEvent(new Event('input', { bubbles: true }));
  });
}

describe('the overlay layer keeps its overlays mounted', () => {
  let root: HTMLDivElement;
  let panel: HTMLDivElement;
  let mount: HTMLDivElement;

  beforeEach(async () => {
    appFullscreenHost.value = null;
    clientRefreshing.value = false;
    root = document.createElement('div');
    document.body.appendChild(root);
    // The fullscreen app panel and its empty mount, as AppUiInline renders them.
    panel = document.createElement('div');
    mount = document.createElement('div');
    mount.setAttribute('data-overlay-layer', '');
    panel.appendChild(mount);
    document.body.appendChild(panel);
    await act(() => {
      render(
        <>
          <div class="app-shell" />
          <OverlayLayer><Toast /><StatefulOverlay /></OverlayLayer>
          <UiBlockingOverlay />
        </>,
        root,
      );
    });
  });

  afterEach(async () => {
    appFullscreenHost.value = null;
    clientRefreshing.value = false;
    await act(() => render(null, root));
    root.remove();
    panel.remove();
  });

  it('keeps the same overlay, and its state, across entering and leaving fullscreen', async () => {
    await type('budget');
    const before = probe();

    await act(() => { appFullscreenHost.value = mount; });
    expect(mount.contains(probe())).toBe(true);
    expect(probe()).toBe(before);
    expect(probe().value).toBe('budget');

    await act(() => { appFullscreenHost.value = null; });
    expect(root.contains(probe())).toBe(true);
    expect(probe()).toBe(before);
    expect(probe().value).toBe('budget');
  });

  it('brings the overlays home before the panel holding them goes away', async () => {
    await act(() => { appFullscreenHost.value = mount; });
    const before = probe();

    // AppUiInline clears the host in its unmount cleanup, which runs before
    // Preact removes the panel's DOM. The overlays must already be home then.
    appFullscreenHost.value = null;
    panel.remove();

    expect(probe()).toBe(before);
    expect(probe().isConnected).toBe(true);
    expect(root.contains(probe())).toBe(true);
  });

  it('keeps the focused field focused, with its caret, across a move', async () => {
    await type('budget');
    probe().focus();
    probe().setSelectionRange(2, 4);

    await act(() => { appFullscreenHost.value = mount; });

    expect(document.activeElement).toBe(probe());
    expect([probe().selectionStart, probe().selectionEnd]).toEqual([2, 4]);
  });

  it('leaves the toast reachable and everything else inert during a refresh', async () => {
    await act(() => { appFullscreenHost.value = mount; });
    await act(() => { clientRefreshing.value = true; });

    expect(root.contains(probe())).toBe(true);
    expect(document.querySelector('.toast-container')!.closest('[inert]')).toBeNull();
    expect(probe().closest('[inert]')).not.toBeNull();
    expect(document.querySelector('.app-shell')!.hasAttribute('inert')).toBe(true);
  });
});
