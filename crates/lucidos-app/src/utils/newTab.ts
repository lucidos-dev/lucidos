/**
 * Opening a browser tab, and knowing whether it happened.
 *
 * A leaf on purpose: it imports nothing. Its two callers report a refusal
 * differently. `openExternalUrl` raises a toast carrying the URL back;
 * `workspaceWindow` rejects to a surface that may have no toast at all.
 */

/** Open `url` in a new tab, reporting whether the browser actually did it.
 *
 *  Deliberately NOT `window.open(url, '_blank', 'noopener')`, so nobody puts
 *  the feature back. With it set, the HTML spec REQUIRES `window.open` to
 *  return null on success exactly as on a block (step 14 of its algorithm).
 *  The return value would then carry no signal. Checked on Chromium and
 *  WebKit, the two engines we ship on.
 *
 *  Severing `opener` re-establishes the half of the feature that is a
 *  reachable path, reverse tabnabbing. The new context is still on its initial
 *  `about:blank` when `window.open` returns, and runs no script until this task
 *  yields. So the reference is gone before the target document exists.
 *
 *  What is NOT re-established is the separate browsing-context group. That
 *  costs isolation depth, not a way back. A `_blank` tab has no name to be
 *  re-targeted by, and it is top-level, so its `top` and `parent` are itself.
 *
 *  Some blockers hand back a window that is already `closed` rather than null,
 *  so both count as blocked. */
export function openNewTab(url: string): boolean {
  const opened = window.open(url, '_blank');
  if (!opened || opened.closed) return false;
  opened.opener = null;
  return true;
}
