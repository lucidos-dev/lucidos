/** Revoke a `blob:` / `data:` object URL, swallowing the synchronous
 *  exception that `URL.revokeObjectURL` throws in non-DOM environments
 *  (vitest's Node runtime) or when the URL was already revoked. */
export function safeRevokeObjectUrl(url: string): void {
  try { URL.revokeObjectURL(url); } catch { /* tests / non-DOM */ }
}
