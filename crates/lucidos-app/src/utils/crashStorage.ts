/** Temporary diagnostics: docs/temporary-measures.md § Page health log. */
import { BASE_PATH } from './basePath';

export const crashStorageKey = (workspace: string) => `lucidos-crash:${workspace}`;

let priorRecord: { raw: string | null } | undefined;

export function preserveCrashRecord(): void {
  if (priorRecord) return;
  priorRecord = { raw: null };
  try {
    const key = crashStorageKey(BASE_PATH);
    const raw = localStorage.getItem(key);
    localStorage.removeItem(key);
    priorRecord.raw = raw && raw.length <= 2300 ? raw : null;
  } catch {  }
}

export function takeCrashRecord(): string | null {
  preserveCrashRecord();
  const raw = priorRecord!.raw;
  priorRecord!.raw = null;
  return raw;
}
