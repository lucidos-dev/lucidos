/**
 * The build-watch's pure helpers: when a build outcome is worth announcing,
 * what to say about a failure, and what goes in the status file.
 *
 * These exist because a wedged frontend build used to be invisible. The atomic
 * publish keeps the previous `dist/`, which is correct and is also why nobody
 * notices. An Apply landed a package the checkout had not installed. Every
 * build then failed for over half an hour, and the only record was a log file.
 * See `docs/plans/2026-08-21-a-wedged-frontend-build-heals-itself-and-shouts.md`.
 *
 * Importing the watcher is safe: it starts a build only when it IS the entry
 * module, which is exactly what makes these testable.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: dev tooling, JS with no type declarations
import * as watcher from '../../dev-build-watch.mjs';
import {
  DEV_BUILD_WATCH_ENV,
  ENTRY_CHUNK_LINE_PREFIX,
  entryChunkLine,
  underDevBuildWatch,
} from '../../vite/entryChunkBudget';

const {
  DEV_BUILD_WATCH_ENV: WATCHER_ENV,
  ENTRY_CHUNK_LINE_PREFIX: WATCHER_PREFIX,
  alertTransition,
  buildChildEnv,
  buildStatusRecord,
  entryChunkAlert,
  entryChunkFromLine,
  firstErrorLine,
} = watcher;

describe('when a build outcome is announced', () => {
  it('speaks on the way into failing, and on the way out', () => {
    expect(alertTransition(true, false)).toBe('broken');
    expect(alertTransition(false, true)).toBe('recovered');
  });

  it('stays quiet while nothing changed', () => {
    // A build fires on every keystroke-sized change. Announcing each failure
    // would be a notification storm, so only the edges speak.
    expect(alertTransition(false, false)).toBeNull();
    expect(alertTransition(true, true)).toBeNull();
  });

  it('announces a first build only when it failed', () => {
    // `null` is the state before this process has built anything. A first build
    // that fails is news. One that succeeds recovered nothing.
    expect(alertTransition(null, false)).toBe('broken');
    expect(alertTransition(null, true)).toBeNull();
  });
});

describe('what a failure says', () => {
  it('takes the line after Rollup names the error', () => {
    const output = [
      'vite v6.4.3 building for production...',
      'transforming...',
      '✗ Build failed in 201ms',
      'error during build:',
      '[vite]: Rollup failed to resolve import "jsqr" from "PairingScanner.tsx".',
      '    at viteLog (file:///node_modules/vite/dist/node/chunks/dep.js:1:1)',
    ].join('\n');
    expect(firstErrorLine(output)).toContain('Rollup failed to resolve import "jsqr"');
  });

  it('falls back to the cross line when there is no Rollup marker', () => {
    expect(firstErrorLine('building...\n✗ Build failed in 12ms\n')).toBe('✗ Build failed in 12ms');
  });

  it('survives a chunk boundary mid-line', () => {
    // The tail is a byte window over piped output, so its first line is
    // routinely half of one. The marker search must still find the real error.
    const truncated = 'ing for production...\ntransforming...\nerror during build:\nthe real error\n';
    expect(firstErrorLine(truncated)).toBe('the real error');
  });

  it('never answers with nothing', () => {
    // An empty message in a notification tells the reader the build broke and
    // nothing else, which is the failure this whole path exists to end.
    expect(firstErrorLine('')).toBeTruthy();
    expect(firstErrorLine('\n\n  \n')).toBeTruthy();
    expect(firstErrorLine('some unrecognised tail')).toBe('some unrecognised tail');
  });
});

describe('the status record', () => {
  const at = '2026-08-21T12:00:00.000Z';

  it('carries the error only when the build failed', () => {
    // A success that kept a stale error would make the engine report a failure
    // that is over.
    expect(buildStatusRecord({ ok: true, at, error: 'stale', skippedInstall: null })).toEqual({
      ok: true,
      at,
      error: null,
      skippedInstall: null,
      entryChunk: null,
    });
  });

  it('keeps a served build green while recording its entry chunk overrun', () => {
    // The overrun is reported, never a failure: `ok: false` would tell the
    // engine nothing new is being served, which is the opposite of true.
    const entryChunk = { fileName: 'assets/index-a.js', bytes: 600_230, budgetBytes: 600_000, overBudget: true };
    const record = buildStatusRecord({ ok: true, at, entryChunk });
    expect(record.ok).toBe(true);
    expect(record.entryChunk).toEqual(entryChunk);
  });

  it('records a refused install alongside the outcome', () => {
    // A build that failed because its dependencies could not be installed has
    // two facts, and the second is the actionable one.
    const record = buildStatusRecord({
      ok: false,
      at,
      error: 'Rollup failed to resolve import "jsqr"',
      skippedInstall: 'a Vite dev server in this checkout holds node_modules',
    });
    expect(record.ok).toBe(false);
    expect(record.error).toContain('jsqr');
    expect(record.skippedInstall).toContain('dev server');
  });

  it('normalises a missing error to null rather than undefined', () => {
    // It is serialised to JSON, and `undefined` would drop the key entirely.
    const record = buildStatusRecord({ ok: false, at });
    expect(record.error).toBeNull();
    expect(JSON.parse(JSON.stringify(record))).toHaveProperty('error');
  });
});

describe('the vite build child', () => {
  it('is told it runs under the watcher, bound to the watcher\'s own pid', () => {
    const env = buildChildEnv({ PATH: '/usr/bin' }, 4242);
    expect(env[WATCHER_ENV]).toBe('4242');
    expect(env.LUCIDOS_ATOMIC_DIST).toBe('1');
    expect(env.PATH).toBe('/usr/bin');
    // The child Vite is spawned directly, so its parent IS the watcher, and the
    // budget plugin accepts the signal only in exactly that case.
    expect(underDevBuildWatch(env, 4242)).toBe(true);
  });

  it('overrides a stale value inherited from whatever started the watcher', () => {
    expect(buildChildEnv({ [WATCHER_ENV]: '1' }, 4242)[WATCHER_ENV]).toBe('4242');
  });

  it('shares its two names with the budget plugin', () => {
    expect(WATCHER_ENV).toBe(DEV_BUILD_WATCH_ENV);
    expect(WATCHER_PREFIX).toBe(ENTRY_CHUNK_LINE_PREFIX);
  });
});

describe('reading the entry chunk measurement', () => {
  it('parses the line the budget plugin prints', () => {
    const line = entryChunkLine({ fileName: 'assets/index-a.js', bytes: 600_230 }, 600);
    expect(entryChunkFromLine(line)).toEqual({
      fileName: 'assets/index-a.js', bytes: 600_230, budgetBytes: 600_000, overBudget: true,
    });
    expect(entryChunkFromLine(entryChunkLine({ fileName: 'a.js', bytes: 500_000 }, 600))?.overBudget)
      .toBe(false);
  });

  it('ignores every other line, and a malformed measurement', () => {
    expect(entryChunkFromLine('dist/assets/index-a.js  600.23 kB')).toBeNull();
    expect(entryChunkFromLine(`${WATCHER_PREFIX}{not json`)).toBeNull();
    expect(entryChunkFromLine(`${WATCHER_PREFIX}{"fileName":"a.js"}`)).toBeNull();
  });

  it('names the size and the budget in the overrun alert', () => {
    const over = { fileName: 'assets/index-a.js', bytes: 600_230, budgetBytes: 600_000, overBudget: true };
    const alert = entryChunkAlert('broken', over);
    expect(alert.title).toContain('over its budget');
    expect(alert.message).toContain('600.23 kB');
    expect(alert.message).toContain('600.00 kB');
    expect(entryChunkAlert('recovered', { ...over, bytes: 520_000, overBudget: false }).title)
      .toContain('back within');
  });
});
