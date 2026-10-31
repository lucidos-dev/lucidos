// @vitest-environment jsdom
import { describe, it, expect, vi, beforeEach } from 'vitest';

const { openExternal } = vi.hoisted(() => ({ openExternal: vi.fn(() => Promise.resolve()) }));
vi.mock('./index', () => ({ lucidos: { ui: { openExternal } } }));
vi.mock('./_bridge', () => ({ isBridged: () => false }));

import { installAppLinkRouting } from './appLinks';

installAppLinkRouting();

/** Click a link and report whether the frame's router took it: cancelled
 *  before the page's own default could navigate the frame. */
function click(href: string): boolean {
  document.body.innerHTML = `<a href="${href}"><b id="t">x</b></a>`;
  const e = new MouseEvent('click', { bubbles: true, cancelable: true });
  const settle = (ev: Event) => ev.preventDefault();
  window.addEventListener('click', settle);
  document.getElementById('t')!.dispatchEvent(e);
  window.removeEventListener('click', settle);
  return openExternal.mock.calls.length > 0;
}

beforeEach(() => openExternal.mockClear());

describe('installAppLinkRouting', () => {
  it.each([
    ['https://example.com/a', 'https://example.com/a'],
    ['HTTPS://example.com/a', 'https://example.com/a'],
    ['Http://example.com/a', 'http://example.com/a'],
    ['//example.com/a', `${window.location.protocol}//example.com/a`],
    ['\\\\example.com/a', `${window.location.protocol}//example.com/a`],
  ])('hands %s to the host', (href, opened) => {
    expect(click(href)).toBe(true);
    expect(openExternal).toHaveBeenCalledWith(opened);
  });

  it('claims the click so the frame never navigates', () => {
    document.body.innerHTML = '<a href="//example.com/a"><b id="t">x</b></a>';
    const e = new MouseEvent('click', { bubbles: true, cancelable: true });
    document.getElementById('t')!.dispatchEvent(e);
    expect(e.defaultPrevented).toBe(true);
  });

  it('leaves a same-site link alone', () => {
    expect(click('/page')).toBe(false);
  });
});
