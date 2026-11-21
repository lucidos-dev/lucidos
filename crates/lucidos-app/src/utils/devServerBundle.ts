/**
 * Is this bundle being served by a Vite dev server rather than built?
 *
 * Since ADR 0014 the workspace is ALWAYS served from a built `dist/`: web-dev,
 * tauri-dev and e2e all run `vite build` and the engine serves the output, so
 * `import.meta.env.DEV` is false everywhere the app normally runs. The two
 * contexts where it is true are a manual `npm run dev` and the **frontend
 * preview**, the `vite serve` the engine supervises from a coding-agent
 * worktree so a TypeScript change is visible before Apply
 * (`crates/lucidos-engine/src/engine/frontend_preview.rs`).
 *
 * One thing must behave differently there, and it is not cosmetic.
 */

/**
 * True when a Vite dev server is serving this page.
 *
 * Not named after the preview, because the flag does not know which of the two
 * dev-server contexts it is in, and both want the same answer.
 */
export function isDevServerBundle(): boolean {
  return import.meta.env.DEV === true;
}

/**
 * Why a dev-server bundle must not register a service worker, phrased for the
 * user because it reaches them through the push toast.
 *
 * Two independent reasons, either sufficient:
 *
 *  - **A dev server emits unhashed module URLs.** `/src/main.tsx` is the same
 *    URL before and after an edit, so a worker that caches it serves the old
 *    module after a hot update. That defeats the entire point of the preview,
 *    and it survives a reload of it.
 *  - **`sw.js` is unstamped here.** Vite serves `public/sw.js` verbatim, with
 *    the literal `__LUCIDOS_BUILD_ID__` the `lucidos-sw-stamp` plugin only
 *    rewrites during `vite build` (see `vite.config.ts`). So the update
 *    machinery keyed on that id would be comparing a placeholder.
 *
 * Push therefore cannot work on a preview either, since it needs a registered
 * worker. That is not a loss worth engineering around: the preview lives on its
 * own origin, so its VAPID scope is separate from the real app's, and a
 * notification delivered there would be delivered to a page the user opened to
 * look at a header.
 */
export const DEV_SERVER_SW_REASON =
  'This page is the frontend preview (a Vite dev server), which registers no service worker, so push cannot be enabled here. Enable it in the real app instead.';
