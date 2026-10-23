/**
 * Tests for panelWebviewVisibilityEffect — the pure hide/show pairing shared by
 * every overlay that can render over the Tauri internal browser (the nav-history
 * popover, Dropdown, Drawer, ConfirmDialog via useHidePanelWebviewWhile). Drives
 * the extracted effect directly with spies — no DOM/Tauri needed (same style as
 * useDelayedLoading.test.ts).
 *
 * Regression: the NavChevron back/forward history popover did NOT hide the
 * native panel webview, so it opened BEHIND the internal browser and looked
 * broken. The webview is a native overlay painting above all HTML; an overlay
 * that wants to be seen over the content pane must hold it hidden while open.
 */
import { describe, it, expect, vi } from 'vitest';
import { panelWebviewVisibilityEffect } from './useHidePanelWebviewWhile';

describe('panelWebviewVisibilityEffect', () => {
  it('hides while active and returns a cleanup that shows again', () => {
    const hide = vi.fn();
    const show = vi.fn();
    const cleanup = panelWebviewVisibilityEffect(true, hide, show);
    expect(hide).toHaveBeenCalledTimes(1);
    expect(show).not.toHaveBeenCalled();
    expect(cleanup).toBeTypeOf('function');
    cleanup!();
    expect(show).toHaveBeenCalledTimes(1);
  });

  it('does nothing while inactive — webview stays visible', () => {
    const hide = vi.fn();
    const show = vi.fn();
    const cleanup = panelWebviewVisibilityEffect(false, hide, show);
    expect(hide).not.toHaveBeenCalled();
    expect(show).not.toHaveBeenCalled();
    expect(cleanup).toBeUndefined();
  });

  it('open → close → reopen hides and shows exactly once each cycle', () => {
    const hide = vi.fn();
    const show = vi.fn();
    // open
    let cleanup = panelWebviewVisibilityEffect(true, hide, show);
    // close (effect re-runs: cleanup, then inactive branch)
    cleanup!();
    panelWebviewVisibilityEffect(false, hide, show);
    // reopen
    cleanup = panelWebviewVisibilityEffect(true, hide, show);
    cleanup!();
    expect(hide).toHaveBeenCalledTimes(2);
    expect(show).toHaveBeenCalledTimes(2);
  });
});
