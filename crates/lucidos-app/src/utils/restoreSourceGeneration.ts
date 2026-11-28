import type { RestoreGeneration } from '../store/restoreGeneration';

export interface RestoreSourceOrigin { sourceGeneration?: RestoreGeneration }
let observed: RestoreGeneration = null;
let admitted = false;
let firstAdmitted: RestoreGeneration = null;

export function sourceGeneration(): RestoreGeneration { return observed; }
export function observeSourceGeneration(generation: RestoreGeneration, sinceLoad = true): void {
  if (!admitted) firstAdmitted = sinceLoad ? generation : null;
  observed = generation;
  admitted = true;
}

export function bindSourceGeneration(): () => RestoreGeneration {
  if (!admitted) return () => firstAdmitted;
  const bound = observed;
  return () => bound;
}

export function isHistoricalRestoreSource(record: RestoreSourceOrigin): boolean {
  return (record.sourceGeneration ?? null) !== observed;
}
