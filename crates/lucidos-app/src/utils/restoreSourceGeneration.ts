/** Page-local origin of durable intent. Never read the shared device marker at
 * write time: another tab may have observed a restore while this page still
 * holds old intent. Legacy records belong to the original (null) generation. */
import type { RestoreGeneration } from '../store/restoreGeneration';

export interface RestoreSourceOrigin { sourceGeneration?: RestoreGeneration }
let observed: RestoreGeneration = null;
let admitted = false;
let firstAdmitted: RestoreGeneration = null;

export function sourceGeneration(): RestoreGeneration { return observed; }
/** `sinceLoad` says the admission compared the hello with the generation read
 * at page load. Without that read, a matching marker may be another tab's. */
export function observeSourceGeneration(generation: RestoreGeneration, sinceLoad = true): void {
  if (!admitted) firstAdmitted = sinceLoad ? generation : null;
  observed = generation;
  admitted = true;
}

/** The generation a request issued now carries. Before this page's first
 * admission it is not known yet. A changed generation reloads the page, so a
 * first admission proven since page load means no restore happened. Such a
 * request takes that generation, or none when the proof is missing. */
export function bindSourceGeneration(): () => RestoreGeneration {
  if (!admitted) return () => firstAdmitted;
  const bound = observed;
  return () => bound;
}

/** Historical records are retained for explicit recovery, never adopted,
 * expired, uploaded or requeued by ordinary startup. */
export function isHistoricalRestoreSource(record: RestoreSourceOrigin): boolean {
  return (record.sourceGeneration ?? null) !== observed;
}
