/** Temporary diagnostics: docs/temporary-measures.md § Page health log. */
import { BASE_PATH } from './basePath';

export const crashStorageKey = (workspace: string) => `lucidos-crash:${workspace}`;

// Undefined means neither startup path has captured the prior document yet.
let priorRecord: { raw: string | null } | undefined;

/** Both startup paths call this before reading or overwriting crash evidence. */
export function preserveCrashRecord(): void {
  if (priorRecord) return;
  priorRecord = { raw: null };
  try {
    const key = crashStorageKey(BASE_PATH);
    const raw = localStorage.getItem(key);
    localStorage.removeItem(key);
    priorRecord.raw = raw && raw.length <= 2300 ? raw : null;
  } catch { /* Storage may be unavailable; the prior snapshot stays empty. */ }
}

/** Consume the boot snapshot once, without touching the current document. */
export function takeCrashRecord(): string | null {
  preserveCrashRecord();
  const raw = priorRecord!.raw;
  priorRecord!.raw = null;
  return raw;
}
