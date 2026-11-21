/**
 * What this window is called, on both surfaces that name it.
 *
 * The two read the same workspace name and differ on purpose. A browser tab
 * sits among other sites, so it keeps the product name and the unread count:
 * `(2) Lucidos - dev`. A native window is already listed under the Lucidos app
 * menu, so it carries the workspace name alone. The Window menu then stops
 * reading `Lucidos` once per window.
 *
 * The name is `visibleWorkspaceName`: the gateway registry's label when we know
 * it, else the engine's directory name. Empty on the picker, and on any page
 * before the health response lands, which is why both composers fall back to
 * the bare product name.
 */

import { isTauri } from './platform';
import { setWindowTitle } from './tauri';

/** The product name on its own: what a window with no workspace is called. */
const PRODUCT = 'Lucidos';

/** The browser tab's title. `unread` is the badge count, and 0 means no prefix.
 *  Callers pass the raw name; a blank one degrades to the product name rather
 *  than to a trailing separator. */
export function documentTitle(workspace: string, unread: number): string {
  const name = workspace.trim();
  const base = name ? `${PRODUCT} - ${name}` : PRODUCT;
  return unread > 0 ? `(${unread}) ${base}` : base;
}

/** The native window's title: the workspace name alone, else the product name.
 *  No unread count, because the Window menu is how a user tells two windows
 *  apart and a moving number is not part of that. */
export function nativeWindowTitle(workspace: string): string {
  return workspace.trim() || PRODUCT;
}

/** The title the shell has taken, so a re-render resolving the same name does
 *  not spend an IPC round trip saying nothing. Empty is "nothing taken yet" and
 *  can never be a composed title.
 *
 *  Recorded once the push RESOLVES, never before. Banking it up front would pin
 *  the record to a name the shell never took, and this same guard would then
 *  refuse to rewrite it. Mirrors the unread indicator in `src/desktop.rs`. */
let applied = '';

/** The push in flight, so at most one is ever outstanding. Two names land in
 *  quick succession on an ordinary load: the engine's own, then the gateway
 *  label. Run concurrently they could resolve out of order, leaving the window
 *  on the older name with nothing left to correct it. */
let queue: Promise<void> = Promise.resolve();

/** Reset the de-duplication. Test seam only: the module state would otherwise
 *  leak one test's value into the next. */
export function resetWindowTitlePush(): void {
  applied = '';
  queue = Promise.resolve();
}

/** Name the calling window after `workspace`. A no-op in the browser, where
 *  there is no native window and `document.title` is the whole story.
 *
 *  The returned promise settles when the shell has answered. Callers ignore it;
 *  it is there so a test can wait. Best-effort telemetry carve-out
 *  (.claude/rules/frontend.md). A toast would be wrong because nothing here is
 *  user-initiated. A failed push leaves the record untouched, so the next name
 *  change retries. */
export function pushNativeWindowTitle(workspace: string): Promise<void> {
  if (!isTauri()) return Promise.resolve();
  const title = nativeWindowTitle(workspace);
  queue = queue.then(() => {
    if (title === applied) return;
    return setWindowTitle(title).then(
      () => {
        applied = title;
      },
      (e: unknown) => console.warn('[window] title not applied', e),
    );
  });
  return queue;
}
