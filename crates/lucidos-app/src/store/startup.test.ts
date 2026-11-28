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

const RESUME_RECONCILED: Array<[call: string, whatBreaks: string]> = [
  ['reg.update()', 'a new frontend build is never picked up by the service worker'],
  ['syncClientUpdateFromBuild(', 'the client-update badge misses a build that landed while away'],
  ['handleResume()', 'the shared gap repair misses engine builds and other state that changed while away'],
  ['refreshReleaseCheckIfShown(', 'a published release goes unannounced until the gateway backstop fires'],
  ['flushPendingPreferenceWrites(', 'a settings change WebKit aborted at suspend never reaches the engine, so the device and the server disagree until the next reload'],
  ['flushUndeliveredComposeDrafts(', 'a draft whose PUT failed while offline is never re-sent, and since a draft lives only in memory it dies with the next iOS eviction'],
];

describe('startClient resume reconciliation', () => {
  const body = handlerBody(readFileSync(SOURCE, 'utf8'), 'function onResume()');

  it('bounds the handler rather than swallowing the whole effect', () => {
    expect(body).not.toContain('startEngineUpdateChecks(');
    expect(body.length).toBeGreaterThan(0);
  });

  it.each(RESUME_RECONCILED)('reconciles %s on resume', (call, whatBreaks) => {
    expect(body, `dropping this call means ${whatBreaks}`).toContain(call);
  });

  it('owns the version-status gap subscription and its teardown', () => {
    const startup = handlerBody(readFileSync(SOURCE, 'utf8'), 'export function startClient()');
    expect(startup).toContain('() => backgroundChecks.refreshEngine(), () => preferencesSettled.value, dismissedEngineVersion);');
    expect(startup).toContain('stopEngineVersionReads()');
  });
});

describe('the global click handler\'s external-link delegation', () => {
  const body = handlerBody(readFileSync(resolve(here, 'globalClick.ts'), 'utf8'), 'function onGlobalClick(');

  it('bounds the handler rather than swallowing the whole effect', () => {
    expect(body).not.toContain('startAppUpdateProgress(');
    expect(body.length).toBeGreaterThan(0);
  });

  it('hands every anchor click to the one external-link router', () => {
    expect(body).toContain('routeExternalAnchorClick(e)');
  });

  it('never opens an external URL itself, so the platform routing has one home', () => {
    expect(body).not.toContain('window.open(');
  });
});

describe('startClient coalesces the iOS wake burst', () => {
  const src = stripComments(readFileSync(SOURCE, 'utf8'));

  it('routes all three wake events through the gate, never at onResume directly', () => {
    for (const binding of [
      `window.addEventListener('focus', onResumeCoalesced)`,
      `window.addEventListener('pageshow', onResumeCoalesced)`,
    ]) {
      expect(src, 'a listener bound straight to onResume re-triples the wake burst').toContain(binding);
    }
    expect(handlerBody(src, 'function handleVisibilityChange()')).toContain('onResumeCoalesced()');
  });

  it('tears down the same references it added, so the listeners cannot leak', () => {
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
    const store = readFileSync(resolve(dirname(SOURCE), '../store/store.ts'), 'utf8');
    expect(store).toContain('localStorage.getItem(NOTIFICATIONS_FILTER_STORAGE_KEY)');
  });
});

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

describe('startShell runs when the shell chunk resolves', () => {
  const main = stripComments(readFileSync(resolve(dirname(SOURCE), '../main.tsx'), 'utf8'));
  const app = stripComments(readFileSync(resolve(dirname(SOURCE), '../App.tsx'), 'utf8'));
  const shell = stripComments(readFileSync(resolve(dirname(SOURCE), '../shellStartup.ts'), 'utf8'));
  const client = stripComments(readFileSync(SOURCE, 'utf8'));
  const effects = stripComments(readFileSync(resolve(dirname(SOURCE), 'effects.ts'), 'utf8'));

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

  it('provides scrollState to the handle before anything else it installs', () => {
    const body = handlerBody(shell, 'export function startShell');
    expect(body.trimStart().startsWith('provideScrollState(scrollState);')).toBe(true);
  });

  const SHELL_INSTALLS = [
    'installDeadPressProbe()',
    'installPressCoverWatch()',
    'installDeadKeystrokeProbe()',
    'installToastPressProbe()',
    'installAppKeybindingsSync()',
    'installAppFocusedThreadSync()',
    'installAppFrameMessages()',
    'installPendingUploadRestore()',
    'installUnsentMessageRestore()',
    'installScaleWheel()',
    'installSeenTargetWatch()',
    'startPageHealthLog()',
    'provideScrollState(scrollState)',
    'watchTranscriptLiveness(',
  ];
  for (const install of SHELL_INSTALLS) {
    it(`installs ${install} there, and nowhere on the entry path`, () => {
      expect(shell).toContain(install);
      expect(client).not.toContain(install);
      expect(effects).not.toContain(install);
      expect(main).not.toContain(install);
    });
  }

  it('subscribes to updater progress before deferring background checks', () => {
    const body = handlerBody(client, 'export function startClient');
    expect(body.indexOf('startAppUpdateProgress()')).toBeGreaterThan(-1);
    expect(body.indexOf('startAppUpdateProgress()')).toBeLessThan(body.indexOf('startDeferredBackgroundChecks()'));
    expect(body).toContain('provideReleaseCheck(async () => { backgroundChecks.refreshRelease(); })');
    expect(shell).not.toContain('startAppUpdateProgress()');
    const background = stripComments(readFileSync(resolve(dirname(SOURCE), 'backgroundChecks.ts'), 'utf8'));
    expect(background).toContain('resumeUpdateRelay()');
    expect(background).toContain('refreshReleaseCheck()');
    expect(client).not.toContain('resumeUpdateRelay()');
  });
});

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
