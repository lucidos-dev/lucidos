/**
 * The frontend preview's API proxy (`crates/lucidos-engine/src/engine/frontend_preview.rs`).
 *
 * The preview is a `vite serve` the engine supervises, rooted in a coding-agent
 * worktree and listening on its OWN port, so a TypeScript or CSS change is
 * visible in the real app before Apply. Its own port means its own origin, and
 * a page on a different origin from the engine would need CORS, which the
 * engine deliberately does not have (one trust boundary, no auth, ADR 0014 §9).
 *
 * So Vite forwards the three engine-owned path prefixes back to the engine, and
 * from the browser's side the preview page is same-origin with its own API. The
 * bundle needs no preview-awareness at all: with no `<base href>` stamped it
 * already takes the `BASE_PATH === ''` branch of `utils/basePath.ts` and builds
 * `/api/v1/…`, which is exactly what lands here.
 *
 * **Inert unless the engine asked for it.** `vite.config.ts`'s `server` block is
 * also what a manual `npm run dev` uses, and pointing that at a dead origin
 * would be a regression for standalone frontend iteration. So the proxy exists
 * only when the engine passed its own origin in
 * `LUCIDOS_FRONTEND_PREVIEW_API_ORIGIN`; without it the resolved config has no
 * `proxy` key at all.
 */

/** Env var the engine sets when it spawns the preview. Mirrored in Rust as
 *  `frontend_preview::PREVIEW_API_ORIGIN_ENV`. */
export const PREVIEW_API_ORIGIN_ENV = 'LUCIDOS_FRONTEND_PREVIEW_API_ORIGIN';

/**
 * The engine-owned path prefixes, and why each one has to be forwarded:
 *   `/api`   every HTTP call and the SSE stream (`/api/v1/…`).
 *   `/app`   app-UI pages, loaded into iframes by relative src.
 *   `/data`  the workspace's `data/` tree: artifacts, app assets, images.
 * Everything else is the bundle itself, which Vite serves.
 */
export const PREVIEW_PROXIED_PREFIXES = ['/api', '/app', '/data'] as const;

interface PreviewProxyEntry {
  target: string;
  changeOrigin: boolean;
  /** The dev engine serves its own self-signed cert; the same
   *  `danger_accept_invalid_certs` allowance every intra-host Lucidos hop makes. */
  secure: boolean;
}

/**
 * The `server.proxy` map for the preview, or `undefined` when this is not a
 * preview (a manual `npm run dev`, a `vite build`, a vitest run).
 *
 * `undefined` rather than `{}` on purpose: spread into the config it must leave
 * no `proxy` key behind, so the standalone dev server is byte-for-byte what it
 * was.
 */
export function frontendPreviewProxy(
  apiOrigin: string | undefined,
): Record<string, PreviewProxyEntry> | undefined {
  const target = apiOrigin?.trim();
  if (!target) return undefined;
  return Object.fromEntries(
    PREVIEW_PROXIED_PREFIXES.map((prefix) => [
      prefix,
      { target, changeOrigin: true, secure: false },
    ]),
  );
}
