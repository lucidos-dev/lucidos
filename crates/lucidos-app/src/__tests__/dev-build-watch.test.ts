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
import { alertTransition, buildStatusRecord, firstErrorLine } from '../../dev-build-watch.mjs';

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
    });
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
