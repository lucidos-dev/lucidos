/**
 * Where inside a workspace an opener asks that workspace to land.
 *
 * Opening a workspace is one thing and landing on a view inside it is another,
 * and the second only ever travels as a URL fragment: the target page is a
 * different document, often one that is not loaded yet.
 * `store/actions/hash-deeplink-router.ts` is the receiving half.
 *
 * A leaf module, importing nothing, so every writer of the hash can share it.
 * `utils/workspaceWindow.ts` already imports `api/client/control.ts`, so a
 * constant living in either of those two would make the other one a cycle.
 *
 * A CLOSED SET, and that is load-bearing rather than tidy. Under the packaged
 * client the name crosses into Rust. There it composes a URL loaded in a window
 * holding the full `window-*` IPC grant (ADR 0028). So the page may name a
 * channel and never supply a string. `window_target::WorkspaceLanding` is the
 * mirror, and its own test pins the same fragment.
 */

/** A view inside a workspace that an opener can ask for by name. */
export type WorkspaceLanding = 'notifications';

/** The fragment each landing is delivered as. The one place the frontend writes
 *  `#notifications`. */
const LANDING_HASH: Record<WorkspaceLanding, string> = {
  notifications: '#notifications',
};

/** The fragment to append to a workspace URL, or `''` for no landing at all. */
export function landingHash(landing?: WorkspaceLanding): string {
  return landing ? LANDING_HASH[landing] : '';
}
