import { describe, it, expect, vi, afterEach } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

// The gateway's boot splash (crates/lucidos-gateway/src/proxy.rs) advances its
// status by polling its own url, not by reloading. A reload repaints the
// whole splash and swaps the words with no transition, so a new boot phase
// must never reload. The script lives in Rust as a string, so
// it is lifted out of the source and run here against a fake document.
const __dirname = dirname(fileURLToPath(import.meta.url));
const proxySource = readFileSync(
  resolve(__dirname, '../../../lucidos-gateway/src/proxy.rs'),
  'utf-8',
);
const poller = proxySource
  .split('const SPLASH_POLLER: &str = r##"<script>')[1]
  ?.split('</script>"##;')[0];

/** What a fetched page says, standing in for its HTML. The fake DOMParser reads it. */
interface Page {
  splash: boolean;
  poll?: string;
  escape?: boolean;
  label?: string;
}

function runPoller(opts: { poll?: string | null; escape?: boolean; motion?: string } = {}) {
  if (!poller) throw new Error('SPLASH_POLLER not found in proxy.rs');
  const statusClasses = new Set<string>();
  const status = {
    textContent: 'Starting engine…',
    classList: { add: (c: string) => statusClasses.add(c), remove: (c: string) => statusClasses.delete(c) },
  };
  const splash = {
    getAttribute: () => (opts.poll === undefined ? '2' : opts.poll),
    querySelector: (sel: string) =>
      sel === '.boot-splash-status' ? status : sel === '.boot-splash-escape' && opts.escape ? {} : null,
  };
  const document = {
    querySelector: () => splash,
    documentElement: { getAttribute: () => opts.motion ?? 'full' },
  };
  const location = { href: 'https://host/myws/', reload: vi.fn() };
  const responses: Array<Page | Error> = [];
  const fetch = vi.fn(() => {
    const next = responses.shift() ?? new Error('no response queued');
    if (next instanceof Error) return Promise.reject(next);
    return Promise.resolve({
      headers: { get: (name: string) => (name === 'x-lucidos-boot-splash' && next.splash ? '1' : null) },
      text: () => Promise.resolve(JSON.stringify(next)),
    });
  });
  class DOMParser {
    parseFromString(html: string) {
      const page = JSON.parse(html) as Page;
      return {
        querySelector: (sel: string) => {
          if (sel === '.boot-splash-status') return { textContent: page.label ?? '' };
          if (sel !== '.boot-splash') return null;
          return {
            getAttribute: () => page.poll ?? null,
            querySelector: () => (page.escape ? {} : null),
          };
        },
      };
    }
  }
  new Function('window', 'document', 'location', 'fetch', 'DOMParser', 'setTimeout', 'clearTimeout', poller)(
    { fetch, DOMParser }, document, location, fetch, DOMParser, setTimeout, clearTimeout,
  );
  return {
    status,
    swapping: () => statusClasses.has('boot-splash-status-swap'),
    location,
    fetch,
    queue: (...pages: Array<Page | Error>) => responses.push(...pages),
  };
}

describe('gateway boot splash poller', () => {
  afterEach(() => vi.useRealTimers());

  it('crossfades a new phase into the same page instead of reloading', async () => {
    vi.useFakeTimers();
    const page = runPoller();
    page.queue({ splash: true, poll: '2', label: 'Recovering sessions…' });
    await vi.advanceTimersByTimeAsync(2000);
    expect(page.fetch).toHaveBeenCalledOnce();
    expect(page.swapping()).toBe(true);
    expect(page.status.textContent).toBe('Starting engine…');
    await vi.advanceTimersByTimeAsync(150);
    expect(page.status.textContent).toBe('Recovering sessions…');
    expect(page.swapping()).toBe(false);
    expect(page.location.reload).not.toHaveBeenCalled();
  });

  it('leaves an unchanged label alone and keeps polling', async () => {
    vi.useFakeTimers();
    const page = runPoller();
    page.queue(
      { splash: true, poll: '2', label: 'Starting engine…' },
      { splash: true, poll: '2', label: 'Starting engine…' },
    );
    await vi.advanceTimersByTimeAsync(4000);
    expect(page.fetch).toHaveBeenCalledTimes(2);
    expect(page.swapping()).toBe(false);
  });

  it('reloads once the engine answers with the app', async () => {
    vi.useFakeTimers();
    const page = runPoller();
    page.queue({ splash: false });
    await vi.advanceTimersByTimeAsync(2000);
    expect(page.location.reload).toHaveBeenCalledOnce();
  });

  it('reloads into the stalled or failed page rather than borrowing its label', async () => {
    vi.useFakeTimers();
    for (const next of [
      { splash: true, poll: '10', escape: true, label: 'This is taking longer than expected.' },
      { splash: true, escape: true, label: 'Lucidos cannot open this workspace.' },
    ]) {
      const page = runPoller();
      page.queue(next);
      await vi.advanceTimersByTimeAsync(2000);
      expect(page.location.reload, JSON.stringify(next)).toHaveBeenCalledOnce();
      expect(page.status.textContent).toBe('Starting engine…');
    }
  });

  it('rides out a failed fetch and tries again', async () => {
    vi.useFakeTimers();
    const page = runPoller();
    page.queue(new Error('offline'), { splash: false });
    await vi.advanceTimersByTimeAsync(2000);
    expect(page.location.reload).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(2000);
    expect(page.location.reload).toHaveBeenCalledOnce();
  });

  it('does nothing on a page with nothing to wait for', async () => {
    vi.useFakeTimers();
    const page = runPoller({ poll: null });
    await vi.advanceTimersByTimeAsync(60_000);
    expect(page.fetch).not.toHaveBeenCalled();
    expect(page.location.reload).not.toHaveBeenCalled();
  });

  it('does not wait out the fade under reduced motion', async () => {
    vi.useFakeTimers();
    const page = runPoller({ motion: 'reduce' });
    page.queue({ splash: true, poll: '2', label: 'Recovering sessions…' });
    await vi.advanceTimersByTimeAsync(2000);
    await vi.advanceTimersByTimeAsync(1);
    expect(page.status.textContent).toBe('Recovering sessions…');
  });
});
