/**
 * Whether the file preview should warn that this script will not run.
 *
 * The engine runs an auth handshake script only if it recorded who wrote it
 * (ADR 0144). Saving one here goes over HTTP, which cannot record, so the
 * script stops running the moment it is edited in the Files panel. Without a
 * warning the user learns that later, from a 502 inside whatever app calls
 * that API.
 *
 * Pure, so the decision is testable without a DOM.
 */

/** One row of `GET /api/v1/handshake-scripts`. */
export interface HandshakeScriptState {
  /** Workspace-relative: `data/scripts/auth/comfort-cloud.py`. */
  path: string;
  exists: boolean;
  approved: boolean;
}

/**
 * The workspace-relative path to warn about, or `null` for every other file.
 *
 * `previewPath` is `data/`-relative (`scripts/auth/x.py`), the same spelling
 * the Files panel and `lucidos.data` use. The API answers in workspace-relative
 * form, so the two are compared after prefixing.
 */
export function handshakeWarningFor(
  previewPath: string,
  scripts: HandshakeScriptState[],
): string | null {
  const key = previewPath.startsWith('data/') ? previewPath : `data/${previewPath}`;
  const match = scripts.find((s) => s.path === key);
  if (!match || !match.exists || match.approved) return null;
  return match.path;
}
