// @vitest-environment jsdom
import { describe, it, expect, vi, beforeEach } from 'vitest';

const { openExternal, toast } = vi.hoisted(() => ({
  openExternal: vi.fn<(url: string) => Promise<void>>(() => Promise.resolve()),
  toast: vi.fn(),
}));
vi.mock('./index', () => ({ lucidos: { ui: { openExternal, toast } } }));
const tellHost = vi.hoisted(() => vi.fn());
vi.mock('./_bridge', () => ({ isBridged: () => false, tellHost, onHostPush: () => () => {} }));

/** The steps the frame told the host, in order. */
function traced(): string[] {
  return tellHost.mock.calls.map((call) => (call[1] as { step: string }).step);
}

import { installAppLinkRouting } from './appLinks';
import { adoptHostExternalLinks } from './externalLinks';

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

beforeEach(() => {
  openExternal.mockReset();
  openExternal.mockImplementation(() => Promise.resolve());
  toast.mockClear();
  tellHost.mockClear();
  vi.restoreAllMocks();
  adoptHostExternalLinks({ frameOpens: 'host' });
});

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

  it('traces the route it chose, after the open', () => {
    const order: string[] = [];
    openExternal.mockImplementation(() => { order.push('open'); return Promise.resolve(); });
    tellHost.mockImplementation(() => { order.push('trace'); });

    click('https://example.com/a');

    expect(order).toEqual(['open', 'trace']);
    expect(tellHost).toHaveBeenCalledWith(
      'log.app-link',
      expect.objectContaining({ step: 'route', route: 'external', href: 'https://example.com/a' }),
    );
  });

  it('leaves a same-site link alone', () => {
    expect(click('/page')).toBe(false);
  });

  describe('when the host route rejects', () => {
    /** Click a web link whose host route fails, and let the catch run. */
    async function clickFailing(href: string): Promise<void> {
      openExternal.mockImplementation(() => Promise.reject(new Error('engine down')));
      vi.spyOn(console, 'warn').mockImplementation(() => {});
      click(href);
      await vi.waitFor(() => expect(openExternal).toHaveBeenCalled());
      await new Promise((r) => setTimeout(r, 0));
    }

    it('opens the page from the frame, opener severed, with no toast', async () => {
      const opened = { closed: false, opener: {} as unknown };
      const open = vi.spyOn(window, 'open').mockReturnValue(opened as unknown as Window);

      await clickFailing('https://example.com/a');

      expect(open).toHaveBeenCalledWith('https://example.com/a', '_blank');
      expect(opened.opener).toBeNull();
      expect(toast).not.toHaveBeenCalled();
      expect(traced()).toEqual(['route', 'host-route-rejected']);
    });

    it('shows an error toast naming the URL when that open is blocked too', async () => {
      vi.spyOn(window, 'open').mockReturnValue(null);

      await clickFailing('https://example.com/a');

      expect(toast).toHaveBeenCalledWith(expect.stringContaining('https://example.com/a'), 'error');
      expect(traced()).toEqual(['route', 'host-route-rejected', 'toast']);
    });

    it('hands the page to Safari, never the in-app overlay, when the user chose Safari', async () => {
      adoptHostExternalLinks({ frameOpens: 'safari' });
      const open = vi.spyOn(window, 'open').mockReturnValue({ closed: false } as unknown as Window);

      await clickFailing('https://example.com/a');

      expect(open).toHaveBeenCalledWith('x-safari-https://example.com/a', '_blank');
      expect(toast).not.toHaveBeenCalled();
    });
  });
});
