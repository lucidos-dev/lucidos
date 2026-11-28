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
    let cleanup = panelWebviewVisibilityEffect(true, hide, show);
    cleanup!();
    panelWebviewVisibilityEffect(false, hide, show);
    cleanup = panelWebviewVisibilityEffect(true, hide, show);
    cleanup!();
    expect(hide).toHaveBeenCalledTimes(2);
    expect(show).toHaveBeenCalledTimes(2);
  });
});
