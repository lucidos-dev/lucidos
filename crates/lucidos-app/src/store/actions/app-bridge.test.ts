// @vitest-environment jsdom
/**
 * The host half of the app bridge, which is the trusted side.
 *
 * jsdom rather than the suite's default stubs, because the sender guard is
 * `contentWindow` identity against the live DOM. A stub `createElement` has no
 * frame to be, so the one check worth testing here could not run at all.
 *
 * An isolated app frame reaches the engine only through this, and the host
 * attaches the user's device id to what it sends. So the cases below are mostly
 * about what it REFUSES: a path outside the SDK's surface, a header that would
 * forge the actor, and a sender that is not a mounted app frame.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync, readdirSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import { APP_REACHABLE_ROUTES, appReachableMethods } from '@lucidos/sdk';
import {
  APP_ID_HEADER,
  THIS_DEVICE,
  fanOutEventFrame,
  installAppBridge,
  pathIsAllowed,
  resolveBridgePath,
  _resetAppBridgeForTesting,
} from './app-bridge';

describe('pathIsAllowed', () => {
  it('admits what the engine classified as app-reachable', () => {
    for (const path of [
      '/threads/list', '/data/artifacts/x.md', '/preferences?key=theme',
      '/apps', '/env-vars', '/models',
    ]) {
      expect(pathIsAllowed(path), path).toBe(true);
    }
    expect(pathIsAllowed('/events/emit', 'POST')).toBe(true);
    expect(pathIsAllowed('/ui/navigate', 'POST')).toBe(true);
    expect(pathIsAllowed('/proxy/sonos/play', 'POST')).toBe(true);
  });

  it('answers per method, not per sub-tree', () => {
    // The prefix list this replaced could not tell these apart: one route, and
    // an app may read it and never write it.
    expect(pathIsAllowed('/models', 'GET')).toBe(true);
    expect(pathIsAllowed('/models', 'DELETE')).toBe(false);
    expect(pathIsAllowed('/preferences', 'PUT')).toBe(true);
    expect(pathIsAllowed('/preferences', 'DELETE')).toBe(false);
  });

  it('admits a bare route, and nothing that merely starts with its name', () => {
    expect(pathIsAllowed('/data')).toBe(true);
    // A plain `startsWith` would take this, and it is a different route.
    expect(pathIsAllowed('/database-dump')).toBe(false);
    expect(pathIsAllowed('/models-registry')).toBe(false);
  });

  it('refuses a route the engine keeps for the host or an agent', () => {
    for (const path of [
      '/credentials', '/credential-value', '/workspaces', '/internal/client-logs',
      '/messages', '/history', '/app/notes/source', '/backup/key',
    ]) {
      expect(pathIsAllowed(path), path).toBe(false);
    }
    expect(pathIsAllowed('/chat/stream', 'POST')).toBe(false);
    expect(pathIsAllowed('/threads/abc/answer-question', 'POST')).toBe(false);
  });

  it('refuses a traversal, plain or percent-encoded', () => {
    // `/api/v1/data/../credentials` resolves to `/api/v1/credentials` at the
    // network layer, so the sub-tree the path was admitted for is not where it
    // lands. The URL parser counts `%2e%2e` as a dot segment too. A scan for a
    // literal `..` therefore admitted the encoded form, and the call still
    // landed on `/credentials`. Resolving first closes both at once.
    expect(pathIsAllowed('/data/../credentials')).toBe(false);
    expect(pathIsAllowed('/data/%2e%2e/credentials')).toBe(false);
    expect(pathIsAllowed('/data/%2E%2E/credentials')).toBe(false);
    expect(pathIsAllowed('/data/.%2e/credentials')).toBe(false);
  });

  it('keeps a `..` that is part of a filename rather than a segment', () => {
    // One segment, not a dot segment, so the URL parser leaves it alone and so
    // must this. Refusing it would break a legitimate artifact path.
    expect(pathIsAllowed('/data/..%2Fcredentials')).toBe(true);
    expect(pathIsAllowed('/data/artifacts/..hidden.md')).toBe(true);
  });

  it('hands back the path it checked, so nothing renormalises afterwards', () => {
    // The caller fetches THIS, which is what makes the check binding.
    expect(resolveBridgePath('/data/artifacts/x.md?v=2')).toBe('/data/artifacts/x.md?v=2');
    expect(resolveBridgePath('/threads/./list')).toBe('/threads/list');
  });

  it('refuses a protocol-relative path, which names another host', () => {
    expect(pathIsAllowed('//example.com/data')).toBe(false);
  });

  it('refuses a path that is not rooted', () => {
    expect(pathIsAllowed('data/x')).toBe(false);
    expect(pathIsAllowed('')).toBe(false);
  });
});

describe('the engine table covers the SDK', () => {
  // A source scan, because the answer lives in the engine and the callers live
  // in a different package. An SDK call the table refuses is a dead feature in
  // every isolated app, and it would fail nowhere else.
  const here = dirname(fileURLToPath(import.meta.url));
  const SDK_SRC = resolve(here, '../../../../../packages/lucidos-sdk/src');

  /**
   * Every rooted path the SDK names, from its string and template literals.
   *
   * Deliberately wider than "the first argument to `request()`". The proxy
   * builds its suffix into a local first. A scan keyed on the call shape missed
   * that, which is the failure mode that makes a guard like this worthless.
   * Quoted literals only, so a regex such as `/^\/+|\/+$/` is not mistaken for
   * a path.
   */
  function sdkPathLiterals(dir: string = SDK_SRC): string[] {
    const paths = new Set<string>();
    // Recursive, because a top-level-only scan is a guard with a blind spot: a
    // file under `src/boot/` would name a path nothing here would check.
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      const full = resolve(dir, entry.name);
      if (entry.isDirectory()) {
        if (entry.name === 'generated') continue;
        for (const p of sdkPathLiterals(full)) paths.add(p);
        continue;
      }
      if (!entry.name.endsWith('.ts') || entry.name.includes('.test.')) continue;
      const source: string = readFileSync(full, 'utf8');
      const re = /[`'"](\/[a-zA-Z0-9/_-]*)/g;
      let m: RegExpExecArray | null;
      while ((m = re.exec(stripComments(source))) !== null) paths.add(m[1]);
    }
    return [...paths];
  }

  /** Drop comments before scanning. The SDK documents `data.edit` with JSON
   *  pointers and the proxy with a sample upstream path, and both look exactly
   *  like an API suffix. Neither is one, and neither is code. */
  function stripComments(source: string): string {
    return source.replace(/\/\*[\s\S]*?\*\//g, '').replace(/^\s*\/\/.*$/gm, '');
  }

  /** A path the SDK turns into a `src` / `href` rather than a fetch. A
   *  subresource loads fine from an opaque origin and never reaches the
   *  bridge, and the engine classifies those as `Asset`. `/api/v1` is the base
   *  the bridge prepends, not a suffix. `/events` is the stream, which the host
   *  holds and relays rather than fetching for an app. `/app/` is an app's own
   *  asset URL, served outside `/api/v1` entirely. */
  const NOT_BRIDGED = ['/static/', '/fonts/', '/api/v1', '/app/'];

  /** The one path the SDK names that no bridged call may take: the stream the
   *  host holds and relays. Matched exactly, because `/events/query` and
   *  `/events/emit` are ordinary bridged calls and must stay covered here. */
  const NOT_BRIDGED_EXACT = ['/events'];

  /** Reachable by SOME method, since a literal carries no verb.
   *
   *  A literal ending in `/` is a template prefix (`'/data/' + segments`)
   *  rather than a whole path, so it is answered by the route it starts. Only
   *  a trailing slash earns that: without it, `/model` would pass on `/models`
   *  and a truncated path would read as covered. */
  function reachableSomehow(literal: string): boolean {
    if (appReachableMethods(literal).length > 0) return true;
    if (!literal.endsWith('/')) return false;
    return APP_REACHABLE_ROUTES.some((r) => r.path.startsWith(literal));
  }

  it('admits every path the SDK asks for', () => {
    const refused = sdkPathLiterals()
      // Slashes alone name no route: `appReach.ts` tests for a leading `//`,
      // which the scan cannot tell from a path.
      .filter((p) => /[a-zA-Z]/.test(p))
      .filter((p) => !NOT_BRIDGED.some((n) => p.startsWith(n)))
      .filter((p) => !NOT_BRIDGED_EXACT.includes(p))
      .filter((p) => !reachableSomehow(p));
    expect(refused, 'the SDK names these and the engine would refuse them').toEqual([]);
  });

  it('spells the device marker the way the SDK spells it', () => {
    // Two copies, because the SDK bundles standalone and cannot import the
    // host. A drift between them fails silently: the host would stop
    // substituting, and every app would read the global preferences again
    // while looking exactly as healthy as it does now.
    const source: string = readFileSync(resolve(SDK_SRC, 'preferences.ts'), 'utf8');
    const declared = /const THIS_DEVICE = '([^']+)'/.exec(source);
    expect(declared, 'the SDK no longer declares THIS_DEVICE').not.toBeNull();
    expect(declared?.[1]).toBe(THIS_DEVICE);
  });

  // Nothing here asserts the table carries ONLY what the SDK asks for: it is
  // wider on purpose, since `lucidos.request` reaches a route with no
  // namespace. What may be in it is the engine's own test to make.
});

describe('the installed router', () => {
  let stop: () => void;
  let frame: HTMLIFrameElement;
  let fetchMock: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    _resetAppBridgeForTesting();
    frame = document.createElement('iframe');
    frame.setAttribute('data-role', 'app-ui-frame');
    // The src the host set is what names the app. Every case needs it now,
    // because the stamp rides on it.
    frame.setAttribute('src', '/app/habit-tracker/');
    document.body.appendChild(frame);
    fetchMock = vi.fn(async () => new Response('{}', { status: 200 }));
    vi.stubGlobal('fetch', fetchMock);
    stop = installAppBridge();
  });

  afterEach(() => {
    stop();
    frame.remove();
    vi.unstubAllGlobals();
  });

  /** Deliver a message as if it came from `source`. jsdom cannot set
   *  `event.source` through `postMessage`, so the event is built by hand. */
  function deliver(source: unknown, data: unknown): void {
    const event = new MessageEvent('message', { data });
    Object.defineProperty(event, 'source', { value: source });
    window.dispatchEvent(event);
  }

  const call = (path: string, headers: Record<string, string> = {}, method = 'GET') => ({
    type: 'lucidos:bridge',
    id: '1',
    op: 'fetch',
    args: { path, method, headers, body: { kind: 'none' }, timeoutMs: 1000 },
  });

  it('answers a mounted app frame', async () => {
    deliver(frame.contentWindow, call('/threads/list'));
    await vi.waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(1));
    expect(String(fetchMock.mock.calls[0][0])).toContain('/threads/list');
  });

  it('ignores a sender that is not a mounted app frame', async () => {
    // A nested frame an app embeds, or any other window. `event.origin` cannot
    // tell them apart once the app frame's origin is opaque, so identity does.
    const stranger = document.createElement('iframe');
    document.body.appendChild(stranger);
    deliver(stranger.contentWindow, call('/threads/list'));
    await new Promise((r) => setTimeout(r, 0));
    expect(fetchMock).not.toHaveBeenCalled();
    stranger.remove();
  });

  it('ignores a message that is not the bridge protocol', async () => {
    deliver(frame.contentWindow, { type: 'lucidos:ui:preview-file', id: 'x' });
    await new Promise((r) => setTimeout(r, 0));
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it('makes no call for a route the engine keeps away from apps', async () => {
    deliver(frame.contentWindow, call('/credentials'));
    await new Promise((r) => setTimeout(r, 0));
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it('makes no call for a method the route does not open', async () => {
    // Read the model registry, never write it. The prefix list could not say
    // this, and the engine's table can.
    deliver(frame.contentWindow, call('/models', {}, 'DELETE'));
    await new Promise((r) => setTimeout(r, 0));
    expect(fetchMock).not.toHaveBeenCalled();

    deliver(frame.contentWindow, call('/models'));
    await vi.waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(1));
  });

  it('names the calling app on the request', async () => {
    // The bridge is the first place that knows which app is calling: the
    // element is the host's, so its src is not the app's claim (ADR 0231).
    deliver(frame.contentWindow, call('/env-vars'));
    await vi.waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(1));
    const sent = fetchMock.mock.calls[0][1].headers as Record<string, string>;
    expect(sent[APP_ID_HEADER]).toBe('habit-tracker');
  });

  it('replaces an app id the app tried to set', async () => {
    deliver(frame.contentWindow, call('/env-vars', { [APP_ID_HEADER]: 'some-other-app' }));
    await vi.waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(1));
    const sent = fetchMock.mock.calls[0][1].headers as Record<string, string>;
    expect(sent[APP_ID_HEADER]).toBe('habit-tracker');
    expect(Object.values(sent)).not.toContain('some-other-app');
  });

  it('relays event-stream frames to a subscriber, and prunes one that unmounted', async () => {
    const posted: unknown[] = [];
    const win = frame.contentWindow as Window;
    vi.spyOn(win, 'postMessage').mockImplementation((m: unknown) => { posted.push(m); });

    deliver(win, { type: 'lucidos:bridge', op: 'sse.open', args: {} });
    await new Promise((r) => setTimeout(r, 0));
    fanOutEventFrame('{"type":"X"}');
    expect(posted).toContainEqual(
      { type: 'lucidos:bridge:push', channel: 'sse', data: { kind: 'frame', data: '{"type":"X"}' } },
    );

    // Unmounted. The subscriber is pruned by `isConnected` on the element kept
    // at subscribe time, never a DOM query: this runs once per streamed frame.
    posted.length = 0;
    frame.remove();
    fanOutEventFrame('{"type":"Y"}');
    expect(posted).toEqual([]);

    document.body.appendChild(frame);
  });

  it('answers a preference read scoped to "this device" with the real id', async () => {
    // Theme, font and scale are device-scoped. The frame cannot read the id,
    // so it names itself and the host fills in the one it stamps anyway.
    localStorage.setItem('lucidos-device-id', 'the-real-one');
    deliver(frame.contentWindow, call('/preferences?device_id=%40device'));
    await vi.waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(1));
    expect(String(fetchMock.mock.calls[0][0])).toContain('/preferences?device_id=the-real-one');
    localStorage.removeItem('lucidos-device-id');
  });

  it('drops the scope rather than sending the marker when no id is minted', async () => {
    deliver(frame.contentWindow, call('/preferences?device_id=%40device'));
    await vi.waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(1));
    expect(String(fetchMock.mock.calls[0][0])).toContain('/preferences');
    expect(String(fetchMock.mock.calls[0][0])).not.toContain('device_id');
  });

  it('substitutes on no other path, so no endpoint can echo the id back', async () => {
    localStorage.setItem('lucidos-device-id', 'the-real-one');
    deliver(frame.contentWindow, call('/threads/list?device_id=%40device'));
    await vi.waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(1));
    expect(String(fetchMock.mock.calls[0][0])).not.toContain('the-real-one');
    localStorage.removeItem('lucidos-device-id');
  });

  it('strips an x-lucidos header, so an app cannot name a device it is not', async () => {
    localStorage.setItem('lucidos-device-id', 'the-real-one');
    deliver(frame.contentWindow, call('/threads/list', { 'X-Lucidos-Device-Id': 'forged' }));
    await vi.waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(1));
    const sent = fetchMock.mock.calls[0][1].headers as Record<string, string>;
    expect(Object.values(sent)).not.toContain('forged');
    expect(sent['x-lucidos-device-id']).toBe('the-real-one');
    localStorage.removeItem('lucidos-device-id');
  });
});
