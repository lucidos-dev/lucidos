// @vitest-environment jsdom
import { describe, it, expect, vi, beforeEach } from 'vitest';

const { openUrl, openWorkspaceThreadUrl, parseWorkspaceThreadUrl } = vi.hoisted(() => ({
  openUrl: vi.fn(),
  openWorkspaceThreadUrl: vi.fn(),
  parseWorkspaceThreadUrl: vi.fn<(url: string) => unknown>(() => null),
}));
vi.mock('./actions/artifacts', () => ({ openUrl }));
vi.mock('./actions/cross-workspace', () => ({ openWorkspaceThreadUrl, parseWorkspaceThreadUrl }));

import { routeExternalAnchorClick } from './externalAnchorClick';

/** Click inside the link, as the shell's document listener sees it. The click
 *  reports whether the router cancelled it, then is cancelled so jsdom does not
 *  try to follow the link. */
function clickOn(href: string | null, { prevented = false } = {}): { claimed: boolean } {
  document.body.innerHTML = href === null ? '<a><b id="t">x</b></a>' : `<a href="${href}"><b id="t">x</b></a>`;
  const e = new MouseEvent('click', { bubbles: true, cancelable: true });
  if (prevented) document.body.addEventListener('click', (ev) => ev.preventDefault(), { once: true });
  let claimed = false;
  const route = (ev: MouseEvent) => routeExternalAnchorClick(ev);
  const record = (ev: Event) => { claimed = ev.defaultPrevented; ev.preventDefault(); };
  document.addEventListener('click', route);
  window.addEventListener('click', record);
  document.getElementById('t')!.dispatchEvent(e);
  document.removeEventListener('click', route);
  window.removeEventListener('click', record);
  return { claimed };
}

beforeEach(() => {
  vi.clearAllMocks();
  parseWorkspaceThreadUrl.mockReturnValue(null);
});

describe('routeExternalAnchorClick', () => {
  it.each([
    ['https://example.com/a', 'https://example.com/a'],
    ['HTTPS://example.com/a', 'https://example.com/a'],
    ['Http://example.com/a', 'http://example.com/a'],
    ['//example.com/a', `${window.location.protocol}//example.com/a`],
    ['\\\\example.com/a', `${window.location.protocol}//example.com/a`],
  ])('claims %s and opens it outside the shell', (href, opened) => {
    expect(clickOn(href).claimed).toBe(true);
    expect(openUrl).toHaveBeenCalledWith(opened);
  });

  it('keeps a workspace thread link inside Lucidos', () => {
    const parsed = { workspace: 'w' };
    parseWorkspaceThreadUrl.mockReturnValue(parsed);
    clickOn('https://example.com/t');
    expect(openWorkspaceThreadUrl).toHaveBeenCalledWith(parsed);
    expect(openUrl).not.toHaveBeenCalled();
  });

  it.each(['/x', '#frag', 'mailto:a@b.c', 'tel:1', null])('leaves %s to the browser', (href) => {
    expect(clickOn(href).claimed).toBe(false);
    expect(openUrl).not.toHaveBeenCalled();
  });

  it('leaves a click an earlier handler already took', () => {
    clickOn('https://example.com/a', { prevented: true });
    expect(openUrl).not.toHaveBeenCalled();
  });
});
