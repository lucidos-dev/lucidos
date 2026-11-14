// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync, readdirSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, join, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { gapClearedProgress, gapRefreshedVersion, repairGapState } from '../eventGap';
import { backupProgress, backupStatusVersion } from '../store';

/**
 * A *gap* is a span in which SSE frames may have been lost: the stream
 * dropped and reopened, or the engine reported `Lagged`. Transient frames are
 * never replayed, so a signal fed only by them stays wrong until something
 * repairs it. A manual backup that finished while an iPhone PWA slept left
 * the Backup page on its old "Last cloud backup" line for exactly this reason.
 *
 * Two halves. The first proves the repair itself. The second makes forgetting
 * a signal fail the build, so the repair cannot rot.
 */

describe('repairing the state a gap leaves behind', () => {
  it('bumps every registered version counter exactly once', () => {
    const version = gapRefreshedVersion();
    const before = version.value;
    repairGapState();
    expect(version.value).toBe(before + 1);
  });

  it('clears every registered progress signal', () => {
    const progress = gapClearedProgress<{ done: number }>();
    progress.value = { done: 3 };
    repairGapState();
    expect(progress.value).toBeNull();
  });

  it('moves the Backup health card off a backup that finished in the gap', () => {
    // The reported bug: BackupCompleted fired while the stream was down, so
    // the page never re-read /backup/status.
    backupProgress.value = { phase: 'upload', progress: 1, total: 2 };
    const before = backupStatusVersion.value;
    repairGapState();
    expect(backupProgress.value).toBeNull();
    expect(backupStatusVersion.value).toBe(before + 1);
  });
});

const SRC: string = resolve(dirname(fileURLToPath(import.meta.url)), '../..');

function sourceFiles(dir: string): string[] {
  return (readdirSync(dir, { withFileTypes: true }) as { name: string; isDirectory(): boolean }[]).flatMap((entry) => {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) return entry.name === 'generated' ? [] : sourceFiles(path);
    return /\.tsx?$/.test(entry.name) && !/\.(test|spec)\.tsx?$/.test(entry.name) ? [path] : [];
  });
}

/** `export const fooVersion = signal(` and `export const fooProgress = gapCleared…(`:
 *  the name, then the callee that builds it. */
const DECLARATION = /export const (\w+(?:Version|Progress)) = (\w+)\b/g;

/** A signal named `*Version` or `*Progress` that SSE frames do not feed, and
 *  why. Each row is a decision. Adding one is the way to pass without
 *  registering, so it should be harder to justify than the factory call. */
const NOT_GAP_STATE: Record<string, string> = {
  engineVersion: 'a version string read from the engine, not a counter',
  latestEngineVersion: 'a version string read from the engine, not a counter',
  latestTauriAppVersion: 'a version string read from the desktop updater, not a counter',
  engineRestartNewVersion: 'a flag set by the restart flow, not a counter',
  appUpdateProgress: 'fed by the desktop updater, not by SSE frames',
  applyAllInProgress: 'a flag the Changes re-read sets, and the gap reconcile already runs it',
  serverComposeVersion: 'bumped by the compose store when its own report merge moves',
  appearanceVersion: 'bumped when this device applies a preference locally',
  codingAgentSessionVersion: 'bumped by loadAllThreads, which the gap reconcile already runs',
};

const FACTORIES = new Set(['gapRefreshedVersion', 'gapClearedProgress']);

function declarations(): Map<string, string> {
  const found = new Map<string, string>();
  for (const file of sourceFiles(SRC)) {
    for (const [, name, callee] of readFileSync(file, 'utf8').matchAll(DECLARATION)) {
      found.set(name, callee);
    }
  }
  return found;
}

describe('every SSE-fed version counter and progress signal is registered', () => {
  const found = declarations();

  it('finds the declarations it guards', () => {
    expect(found.get('backupStatusVersion')).toBe('gapRefreshedVersion');
    expect(found.get('backupProgress')).toBe('gapClearedProgress');
  });

  it('leaves none built without a factory unless it says why', () => {
    const unregistered = [...found]
      .filter(([name, callee]) => !FACTORIES.has(callee) && !(name in NOT_GAP_STATE))
      .map(([name]) => name);
    expect(unregistered).toEqual([]);
  });

  it('keeps no exemption for a signal that is gone or now registered', () => {
    const stale = Object.keys(NOT_GAP_STATE).filter((name) => {
      const callee = found.get(name);
      return callee === undefined || FACTORIES.has(callee);
    });
    expect(stale).toEqual([]);
  });
});
