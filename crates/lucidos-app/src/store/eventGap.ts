import { signal, type Signal } from '@preact/signals';


const versions: Signal<number>[] = [];
const progresses: Signal<unknown>[] = [];

export function gapRefreshedVersion(): Signal<number> {
  const version = signal(0);
  versions.push(version);
  return version;
}

export function gapClearedProgress<T>(): Signal<T | null> {
  const progress = signal<T | null>(null);
  progresses.push(progress);
  return progress;
}

export function repairGapState(): void {
  for (const version of versions) version.value++;
  for (const progress of progresses) progress.value = null;
}
