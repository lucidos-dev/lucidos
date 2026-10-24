import { signal, type Signal } from '@preact/signals';

/**
 * Signals fed only by transient SSE frames, registered where they are created.
 *
 * The stream never replays a dropped frame. A signal only a frame can move
 * therefore stays wrong after a gap: a reopen after an error, or `Lagged`.
 * Each factory registers its signal, and `repairGapState` repairs all of them.
 * A signal built without a factory is not repaired. The source scan in
 * `__tests__/an-event-gap-repairs-every-sse-fed-signal.test.ts` fails the
 * build for it.
 *
 * Standalone, not in `store.ts`, so a test that mocks `../store` still reaches
 * the same registry.
 */

const versions: Signal<number>[] = [];
const progresses: Signal<unknown>[] = [];

/** A version counter a page re-reads on. A gap bumps it once. */
export function gapRefreshedVersion(): Signal<number> {
  const version = signal(0);
  versions.push(version);
  return version;
}

/** Ephemeral progress only SSE frames set. `null` means no run is going. A gap
 *  clears it, and the page then reads the running state from the engine. */
export function gapClearedProgress<T>(): Signal<T | null> {
  const progress = signal<T | null>(null);
  progresses.push(progress);
  return progress;
}

export function repairGapState(): void {
  for (const version of versions) version.value++;
  for (const progress of progresses) progress.value = null;
}
