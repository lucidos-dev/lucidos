// @vitest-environment jsdom
import { describe, it, expect, vi, afterEach, beforeEach } from 'vitest';
import { installTooltips, TOOLTIP_HOVER_DELAY_MS } from './tooltip';

function hover(target: Element) {
  target.dispatchEvent(new MouseEvent('mouseover', { bubbles: true, clientX: 10, clientY: 10 }));
  vi.advanceTimersByTime(TOOLTIP_HOVER_DELAY_MS + 1);
}

function tooltipShown(): boolean {
  return document.querySelector<HTMLElement>('#tooltip')?.style.opacity === '1';
}

describe('a toggle whose menu is open', () => {
  let uninstall: () => void = () => {};

  beforeEach(() => {
    vi.useFakeTimers();
    delete (window as { ontouchstart?: unknown }).ontouchstart;
    uninstall = installTooltips();
  });

  afterEach(() => {
    uninstall();
    document.body.innerHTML = '';
    vi.useRealTimers();
  });

  function toggle(expanded: boolean, popup = true): HTMLButtonElement {
    const button = document.createElement('button');
    if (popup) button.setAttribute('aria-haspopup', 'menu');
    button.setAttribute('aria-expanded', String(expanded));
    button.setAttribute('data-tooltip-title', 'Menu');
    button.setAttribute('data-tooltip-rows', JSON.stringify([{ label: 'Unread', value: '3' }]));
    button.innerHTML = '<span class="glyph"></span>';
    document.body.appendChild(button);
    return button;
  }

  it('shows its tooltip while the menu is closed', () => {
    hover(toggle(false).querySelector('.glyph')!);
    expect(tooltipShown()).toBe(true);
  });

  it('shows no tooltip over the open menu', () => {
    hover(toggle(true).querySelector('.glyph')!);
    expect(tooltipShown()).toBe(false);
  });

  it('hides a shown tooltip when the toggle is activated from the keyboard', () => {
    const button = toggle(false);
    hover(button.querySelector('.glyph')!);
    button.click();
    expect(tooltipShown()).toBe(false);
  });

  it('keeps the tooltip on an expanded inline disclosure, which covers nothing', () => {
    hover(toggle(true, false).querySelector('.glyph')!);
    expect(tooltipShown()).toBe(true);
  });
});
