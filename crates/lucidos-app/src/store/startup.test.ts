/**
 * Regression guard for the resume-reconciliation set.
 *
 * `startClient`'s `onResume` is what a long-resident client relies on to notice
 * anything that changed while the user was away: it fires on window `focus`,
 * `visibilitychange` and `pageshow`. Several separate update surfaces reconcile
 * there (the list below is the set, so it cannot go stale against a count in
 * this sentence), and each one is a single unremarkable call that a refactor can
 * drop without breaking a type or a test. Losing one is invisible until a user
 * asks why they were never told about a release.
 *
 * That is not hypothetical. The packaged app updater was missing from this set
 * until 2026-07-31, so a 0.18.0 client that neither remounted nor waited out its
 * poll interval reported itself current for a whole morning with 0.18.2 already
 * published (`store/actions/app-update.ts`). This test exists so the set can only
 * shrink deliberately.
 *
 * A source scan rather than a live start: `startClient` wires SSE, service
 * workers, timers, presence and push, so standing it up in jsdom to observe one
 * call would cost far more than the invariant is worth, and would pin the
 * mechanism instead of the requirement.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const SOURCE = resolve(here, 'startup.ts');

/** Strip `//` and block comments so a surviving comment can never stand in for
 *  a deleted call. Dropping the call and leaving the prose that explains it is
 *  the exact shape this guard has to catch.
 *
 *  A `//` preceded by a backslash is left alone: that is the tail of a regex
 *  literal such as `/^https?:\/\//`, whose escaped slash and closing delimiter
 *  read as a line comment and would otherwise swallow the rest of the line
 *  (taking the scheme test the external-link guard asserts on with it). */
function stripComments(src: string): string {
  return src.replace(/\/\*[\s\S]*?\*\//g, '').replace(/(^|[^\\])\/\/.*$/gm, '$1');
}

/** The body of a `function <name>(…)` declaration, by brace matching from its
 *  opening `{`. Comments are stripped first, so a brace inside one cannot skew
 *  the match. */
function handlerBody(src: string, declaration: string): string {
  const stripped = stripComments(src);
  const start = stripped.indexOf(declaration);
  expect(start, `startup.ts must declare \`${declaration}\``).toBeGreaterThan(-1);
  const open = stripped.indexOf('{', start);
  let depth = 0;
  for (let i = open; i < stripped.length; i++) {
    if (stripped[i] === '{') depth++;
    else if (stripped[i] === '}' && --depth === 0) return stripped.slice(open + 1, i);
  }
  throw new Error(`unbalanced braces in \`${declaration}\`, so the guard cannot bound the handler`);
}

/** Every update surface that MUST be reconciled when the page comes back to the
 *  foreground, and what silently rots if its call goes missing. */
const RESUME_RECONCILED: Array<[call: string, whatBreaks: string]> = [
  ['reg?.update()', 'a new frontend build is never picked up by the service worker'],
  ['syncClientUpdateFromBuild(', 'the client-update badge misses a build that landed while away'],
  ['checkEngineVersion(', 'an engine build that finished while away shows as still spinning'],
  ['refreshReleaseCheck(', 'a published release goes unannounced until the gateway backstop fires'],
  ['flushPendingPreferenceWrites(', 'a settings change WebKit aborted at suspend never reaches the engine, so the device and the server disagree until the next reload'],
  ['flushUndeliveredComposeDrafts(', 'a draft whose PUT failed while offline is never re-sent, and since a draft lives only in memory it dies with the next iOS eviction'],
];

describe('startClient resume reconciliation', () => {
  const body = handlerBody(readFileSync(SOURCE, 'utf8'), 'function onResume()');

  // Proves the brace match actually bounded the handler.
  // `startAppUpdateProgress` is called once in the effect body and never on
  // resume. Seeing it here would mean the slice overran, and every assertion
  // below had become vacuous.
  it('bounds the handler rather than swallowing the whole effect', () => {
    expect(body).not.toContain('startAppUpdateProgress(');
    expect(body.length).toBeGreaterThan(0);
  });

  it.each(RESUME_RECONCILED)('reconciles %s on resume', (call, whatBreaks) => {
    expect(body, `dropping this call means ${whatBreaks}`).toContain(call);
  });
});

/**
 * The delegated anchor route in `onGlobalClick` is the ONLY thing standing
 * between a plain `<a href="https://…">` and the browser's own navigation.
 * Chat markdown (`renderMarkdown` / `linkifyPaths`), the rendered markdown file
 * preview and the settings rows all emit raw `target="_blank"` anchors with no
 * component handler of their own, so this handler is what funnels every one of
 * them into `openUrl`.
 *
 * That funnel is load-bearing beyond tidiness: on an installed iOS PWA a
 * `target="_blank"` anchor opens the inescapable in-app web view, and `openUrl`
 * is where the `x-safari-` hand-off lives (`utils/openExternalUrl.ts`). Delete
 * the branch and the platform fix silently stops reaching the surfaces the user
 * actually taps. Same source-scan reasoning as the resume guard above.
 */
describe('startClient external-link delegation', () => {
  const body = handlerBody(readFileSync(SOURCE, 'utf8'), 'function onGlobalClick(');

  it('bounds the handler rather than swallowing the whole effect', () => {
    expect(body).not.toContain('startAppUpdateProgress(');
    expect(body.length).toBeGreaterThan(0);
  });

  it('claims every http(s) anchor and routes it through openUrl', () => {
    expect(body).toContain(`closest('a[href]')`);
    expect(body).toContain('/^https?:\\/\\//.test(href)');
    expect(body).toContain('preventDefault()');
    expect(body).toContain('openUrl(href)');
  });

  it('never opens an external URL itself, so the platform routing has one home', () => {
    expect(body).not.toContain('window.open(');
  });
});

/**
 * One wake, one pass.
 *
 * iOS fires `visibilitychange`, `focus` AND `pageshow` together on a resume, so
 * binding `onResume` to all three ran the whole reconciliation set above three
 * times per wake. The gateway log showed it directly: 3x `engine/version-status`,
 * 3x `memory/embedding-model-status` and 3-4x `notifications` inside one second.
 *
 * That is worse than wasted requests. The burst goes down a tunnel that is
 * itself still re-establishing after the wake, and it grows with the workspace
 * (85 thread-event GETs in one minute, against 16 earlier the same day). It also
 * silently collapsed the tolerance of every consecutive-failure counter reached
 * from here: `loadUnreadNotifications` is meant to stay quiet until three
 * failures in a row, and one bad wake was spending all three on its own, which
 * is why the stale-unread-count card appeared constantly.
 *
 * A source scan for the same reason as the guard above: standing `startClient` up
 * in jsdom to count listener invocations would pin the mechanism rather than the
 * requirement. The gate's own behaviour is unit-tested in
 * `utils/leadingEdgeGate.test.ts`.
 */
describe('startClient coalesces the iOS wake burst', () => {
  const src = stripComments(readFileSync(SOURCE, 'utf8'));

  it('routes all three wake events through the gate, never at onResume directly', () => {
    for (const binding of [
      `window.addEventListener('focus', onResumeCoalesced)`,
      `window.addEventListener('pageshow', onResumeCoalesced)`,
    ]) {
      expect(src, 'a listener bound straight to onResume re-triples the wake burst').toContain(binding);
    }
    // The visibility branch reaches it through its own handler.
    expect(handlerBody(src, 'function handleVisibilityChange()')).toContain('onResumeCoalesced()');
  });

  it('tears down the same references it added, so the listeners cannot leak', () => {
    // `removeEventListener` matches on function identity: leaving these pointing
    // at `onResume` would silently keep the old listeners alive across restarts,
    // and each restart would add a fresh set on top.
    expect(src).toContain(`window.removeEventListener('focus', onResumeCoalesced)`);
    expect(src).toContain(`window.removeEventListener('pageshow', onResumeCoalesced)`);
    expect(src).not.toContain(`window.removeEventListener('focus', onResume)`);
    expect(src).not.toContain(`window.removeEventListener('pageshow', onResume)`);
  });

  it('gates on the shared leading-edge helper rather than a bespoke timer', () => {
    const gate = handlerBody(src, 'function onResumeCoalesced()');
    expect(gate).toContain('resumeGate.allow()');
    expect(src).toContain('createLeadingEdgeGate(RESUME_COALESCE_MS)');
  });
});

/**
 * The app-facing toast bridge reaches the handler at all.
 *
 * `lucidos.ui.dismissToast(key)` posts a payload carrying a key and NO message,
 * which puts it between two filters that would each swallow it silently and
 * identically: the message-type allow-list at the top of `onAppFrameMessage`,
 * and the "confirm and prompt carry a message" early return further down. Either
 * one drops the message with no error anywhere, so from the app's side a dismiss
 * simply does nothing and the spinner it was meant to clear spins forever.
 *
 * What the bridge DOES once reached is behaviour, tested as behaviour in
 * `components/shared/__tests__/toast-app-bridge.test.tsx`. Only the two ordering
 * facts that live in this file are pinned here, for the same reason as the
 * guards above: the routing is unreachable without starting the whole client.
 */
describe('startClient app toast bridge wiring', () => {
  const src = stripComments(readFileSync(SOURCE, 'utf8'));
  const body = handlerBody(src, 'function onAppFrameMessage(');

  it('admits the dismiss message type past the allow-list', () => {
    expect(body, 'a type missing from the allow-list never reaches any branch')
      .toContain(`data.type !== 'lucidos:ui:dismissToast'`);
  });

  it('routes the toast bridge BEFORE the message guard that would swallow a dismiss', () => {
    const bridgeAt = body.indexOf('handleAppToastMessage(');
    const guardAt = body.indexOf(`typeof payload.message !== 'string'`);
    expect(bridgeAt, 'the toast bridge must be wired into the handler').toBeGreaterThan(-1);
    expect(guardAt).toBeGreaterThan(-1);
    expect(guardAt, 'a dismiss carries no message, so the guard must come second').toBeGreaterThan(bridgeAt);
  });

  it('keeps the frame-authenticity check ahead of both', () => {
    // An unattributed frame gets no host chrome, whatever it asked for: a nested
    // embed must not be able to clear a toast the real app is showing.
    const frameAt = body.indexOf('isKnownAppFrame(source)');
    expect(frameAt).toBeGreaterThan(-1);
    expect(body.indexOf('handleAppToastMessage(')).toBeGreaterThan(frameAt);
  });
});

/**
 * Cold-starting on the Notifications panel does not wait out the preferences
 * round-trip before asking for the inbox.
 *
 * Chaining the list load onto `loadPreferences()` held the panel empty for one
 * whole round-trip, before the one that would fill it even began. What it
 * guarded was the filter: read from the server rather than from its cache. The
 * load goes first now, and the served filter corrects it where the two
 * disagree, which is a tab switched on another device.
 *
 * That chaining was itself a fix. A list loaded under 'all' while the toggle
 * then flipped to 'unread', so read rows showed under Unread. Two later changes
 * retired it. The filter signal is seeded from its cache when the module loads,
 * so no effect outruns it. And the Unread tab renders `unreadNotifications`, so
 * a browse list cannot surface there whatever filter fetched it.
 */
describe('startClient notifications cold start', () => {
  const src = stripComments(readFileSync(SOURCE, 'utf8'));
  const eagerAt = src.indexOf('void loadNotifications()');
  const preferencesAt = src.indexOf('loadPreferences().then(');

  it('asks for the inbox before the preferences round-trip, not inside it', () => {
    expect(eagerAt, 'the cold-start inbox load must exist').toBeGreaterThan(-1);
    expect(preferencesAt).toBeGreaterThan(-1);
    expect(eagerAt, 'a load inside the .then() costs the panel a whole round-trip of blank')
      .toBeLessThan(preferencesAt);
  });

  it('still corrects the tab when the served filter disagrees with the cache', () => {
    expect(src).toContain('const filterBeforePreferences = notificationsFilter.value');
    expect(src).toContain('notificationsFilter.value !== filterBeforePreferences');
    expect(src, 're-sourcing must cover whichever tab won, not just the browse list')
      .toContain('refreshActiveNotificationsTab()');
  });

  it('rests on a filter the store seeds before any effect runs', () => {
    // The precondition for loading first. Seed the signal from the served
    // value's cache at module load and this effect cannot outrun it. Move the
    // seed into an effect or an await and the eager load starts guessing.
    const store = readFileSync(resolve(dirname(SOURCE), '../store/store.ts'), 'utf8');
    expect(store).toContain(`localStorage.getItem('lucidos-notifications-filter')`);
  });
});

/**
 * The ordering that makes the shell chunk a real first-paint split (ADR 0288).
 * `boot()` starts the client before it renders the lazy shell, so the startup
 * fetches run while the shell chunk downloads and parses. Moved back into the
 * UI tree, every fetch would wait for the whole UI again. The entry chunk
 * budget cannot see that.
 */
describe('startClient runs before the shell renders', () => {
  const main = stripComments(readFileSync(resolve(dirname(SOURCE), '../main.tsx'), 'utf8'));
  const app = stripComments(readFileSync(resolve(dirname(SOURCE), '../App.tsx'), 'utf8'));

  it('is called by boot() ahead of render()', () => {
    const startAt = main.indexOf('startClient()');
    expect(startAt, 'main.tsx must start the client').toBeGreaterThan(-1);
    expect(startAt).toBeLessThan(main.indexOf('render('));
  });

  it('is not started from inside the UI tree', () => {
    expect(app).not.toContain('startClient');
  });

  it('asks for the shell chunk before boot() awaits anything', () => {
    const preloadAt = main.indexOf('App.preload()');
    expect(preloadAt, 'main.tsx must preload the shell chunk').toBeGreaterThan(-1);
    expect(preloadAt).toBeLessThan(main.indexOf('async function boot('));
  });
});
