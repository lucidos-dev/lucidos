/**
 * Opening a browser tab, and knowing whether it happened.
 *
 * A leaf on purpose: it imports nothing. Its two callers report a refusal
 * differently. `openExternalUrl` raises a toast carrying the URL back;
 * `workspaceWindow` rejects to a surface that may have no toast at all.
 *
 * They differ in one more way. A workspace is a place the user returns to, so
 * its tab is NAMED and a second activation lands in the tab already open. An
 * external URL is not, so it takes a fresh `_blank` tab every time.
 */

/** Open `url` in another tab, reporting whether the browser actually did it.
 *  A named `target` is re-targeted: the browser navigates the tab already open
 *  under that name and fronts it, instead of stacking a second. A blocker may
 *  hand back a window already `closed`, so both that and null count as blocked.
 *
 *  Deliberately NOT `window.open(url, target, 'noopener')`, so nobody puts the
 *  feature back. With it set, the HTML spec REQUIRES `window.open` to return
 *  null on success exactly as on a block (step 14 of its algorithm). The
 *  return value would then carry no signal. It would also put the tab in its
 *  own browsing-context group, where a name can never find it again.
 *
 *  Severing `opener` re-establishes the half of that feature which is a
 *  reachable path, reverse tabnabbing. The new context is still on its initial
 *  `about:blank` when `window.open` returns, and runs no script until this task
 *  yields. So the reference is gone before the target document exists.
 *
 *  The separate browsing-context group is NOT re-established. That costs
 *  isolation depth rather than a way back: the tab is top-level, so `top` and
 *  `parent` are itself. Keeping it is what lets a name find the tab.
 *
 *  Severing is `_blank` ONLY, because only there is the tab certainly ours. A
 *  named target may resolve to one this call did not create, whose `opener` is
 *  not ours to drop. Every named target we use loads a Lucidos page: this
 *  gateway, or a peer engine on this host. No untrusted document holds the
 *  reference. A peer engine on its own port is a different ORIGIN and does keep
 *  one, which is accepted: it is first-party, not an attacker. */
export function openNewTab(url: string, target = '_blank'): boolean {
  const opened = window.open(url, target);
  if (!opened || opened.closed) return false;
  if (target === '_blank') opened.opener = null;
  return true;
}
