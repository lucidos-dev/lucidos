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
import { handlerBody, stripComments } from './__tests__/sourceScan';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const SOURCE = resolve(here, 'startup.ts');

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
    expect(store).toContain('localStorage.getItem(NOTIFICATIONS_FILTER_STORAGE_KEY)');
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

/**
 * Shell startup installs what only a drawn UI can need, and it must finish
 * before `<App/>` first renders. The loader that resolves the shell chunk is
 * the one place that holds for: `lazyComponent` renders only what it returns.
 */
describe('startShell runs when the shell chunk resolves', () => {
  const main = stripComments(readFileSync(resolve(dirname(SOURCE), '../main.tsx'), 'utf8'));
  const app = stripComments(readFileSync(resolve(dirname(SOURCE), '../App.tsx'), 'utf8'));
  const shell = stripComments(readFileSync(resolve(dirname(SOURCE), '../shellStartup.ts'), 'utf8'));
  const client = stripComments(readFileSync(SOURCE, 'utf8'));

  it('is called in the shell loader, before it hands <App/> back', () => {
    const loader = main.slice(main.indexOf("import('./App')"));
    const startAt = loader.indexOf('m.startShell()');
    expect(startAt, 'the shell loader must start the shell').toBeGreaterThan(-1);
    expect(startAt).toBeLessThan(loader.indexOf('return m.App'));
  });

  it('ships in the shell chunk, not the entry', () => {
    expect(app).toContain("export { startShell } from './shellStartup'");
    expect(main).not.toMatch(/import [^;]*from '\.\/shellStartup'/);
  });

  // Each of these once sat in the entry chunk through client startup or
  // main.tsx. Moving one back costs its whole subtree on first paint.
  const SHELL_INSTALLS = [
    'installDeadPressProbe()',
    'installDeadKeystrokeProbe()',
    'installToastPressProbe()',
    'installAppKeybindingsSync()',
    'installAppFrameMessages()',
    'installPendingUploadRestore()',
    'installUnsentMessageRestore()',
  ];
  for (const install of SHELL_INSTALLS) {
    it(`installs ${install} there, and nowhere on the entry path`, () => {
      expect(shell).toContain(install);
      expect(client).not.toContain(install);
      expect(main).not.toContain(install);
    });
  }
});

/**
 * Events store a device's id, never its name, so every screen that names a
 * device reads the devices list. It must load on every boot, not only when the
 * restored tab is Settings.
 */
describe('startClient device lists', () => {
  const src = stripComments(readFileSync(SOURCE, 'utf8'));

  it('loads the devices list whatever tab the app restores to', () => {
    const settingsBranch = src.indexOf("if (tab === 'settings')");
    const load = src.indexOf('loadDevices();');
    expect(load, 'startup must load the devices list').toBeGreaterThan(-1);
    expect(settingsBranch).toBeGreaterThan(-1);
    expect(load, 'the load must not sit inside the Settings-only branch').toBeLessThan(settingsBranch);
  });

  it('loads the pairing list too, since a pairing label is part of a name', () => {
    const settingsBranch = src.indexOf("if (tab === 'settings')");
    const load = src.indexOf('void loadPairedDevices();');
    expect(load, 'startup must load the pairing list').toBeGreaterThan(-1);
    expect(load).toBeLessThan(settingsBranch);
  });
});
