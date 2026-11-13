import { describe, it, expect, vi, afterEach } from 'vitest';

const tellHost = vi.hoisted(() => vi.fn());
vi.mock('./_bridge', () => ({ tellHost, onHostPush: () => () => {} }));

import { adoptHostExternalLinks, currentFrameOpens, traceAppLink } from './externalLinks';

afterEach(() => {
  adoptHostExternalLinks(null);
  tellHost.mockReset();
});

describe('adoptHostExternalLinks', () => {
  it.each(['tab', 'safari', 'host'] as const)('takes the %s answer', (mode) => {
    adoptHostExternalLinks({ frameOpens: mode });
    expect(currentFrameOpens()).toBe(mode);
  });

  it.each([undefined, null, {}, { frameOpens: 'chrome' }, { opensInFrame: true }])(
    'treats %j as the host route',
    (push) => {
      adoptHostExternalLinks({ frameOpens: 'tab' });
      adoptHostExternalLinks(push);
      expect(currentFrameOpens()).toBe('host');
    },
  );
});

describe('traceAppLink', () => {
  it('tells the host the step and its detail', () => {
    traceAppLink('route', { href: 'https://example.com/a' });
    expect(tellHost).toHaveBeenCalledWith('log.app-link', { step: 'route', href: 'https://example.com/a' });
  });

  it('shortens a long value so the engine accepts the line', () => {
    traceAppLink('route', { href: `data:text/html,${'x'.repeat(10_000)}` });
    const sent = tellHost.mock.calls[0][1] as { href: string };
    expect(sent.href.length).toBeLessThan(300);
  });

  it('swallows a failing host, so a tap never breaks', () => {
    tellHost.mockImplementation(() => { throw new Error('no host'); });
    expect(() => traceAppLink('route')).not.toThrow();
  });
});
