// App-icon badge (Badging API). Sets the unread count on the installed PWA's
// icon. Shared by the workspace context (see `syncWorkspaceAppBadge` below) and
// the gateway picker (aggregate total across running workspaces).
//
// Best-effort + feature-detected: browsers without the Badging API, and the
// Tauri WKWebView, which uses a native dock badge instead, simply no-op. A
// non-positive count clears the badge; a positive count sets the number.

import { computed, signal } from '@preact/signals';
import { unreadCount } from '../store';
import { IS_PICKER, WORKSPACE_ID } from '../../utils/basePath';
import { isTauri } from '../../utils/platform';
import { listWorkspaces, type WorkspaceStatus } from '../../api/client/control';

type BadgingNavigator = Navigator & {
  setAppBadge?: (count?: number) => Promise<void>;
  clearAppBadge?: () => Promise<void>;
};

/** Mirror `count` onto the installed PWA's app-icon badge. No-op when the
 *  Badging API is unavailable. `count <= 0` clears the badge. */
export function applyAppBadge(count: number): void {
  const nav = navigator as BadgingNavigator;
  if (typeof nav.setAppBadge !== 'function') return;
  if (count > 0) {
    nav.setAppBadge(count).catch(() => {});
  } else {
    nav.clearAppBadge?.().catch(() => {});
  }
}

/** Every workspace this gateway serves, as of the last refresh. Empty until the
 *  first listing lands, and empty outside a gateway-served workspace context.
 *
 *  The ROWS rather than a sum, because two surfaces read them. The icon badge
 *  wants the total. The Lucidos menu's notifications group wants one row per
 *  workspace holding unreads. One poll feeds both, so the two can never
 *  disagree about a workspace's count.
 *
 *  A signal, so the effect in `store/effects.ts` subscribes through the
 *  computeds below and re-asserts the icon when a refresh moves the total. */
export const peerWorkspaces = signal<WorkspaceStatus[]>([]);

/** Monotonic id of the newest refresh issued, so a slower earlier listing can
 *  never land on top of a fresher one. Three triggers can overlap (startup, a
 *  resume, the visible-only interval), and the gateway's listing takes as long
 *  as its slowest stack lock, so "the last response wins" would leave the icon
 *  on stale counts until the next refresh. Same guard `loadUnreadNotifications`
 *  uses for the unread set (`notifications.ts`). */
let refreshSeq = 0;

/** True when this page is served by the workspace gateway under `/<slug>/`, and
 *  therefore when the icon badge is a CROSS-WORKSPACE total.
 *
 *  The gateway re-stamps every manifest it serves with `scope: "/"`
 *  (`gateway_manifest_json`), so ONE installed icon covers the picker and every
 *  `/<slug>/` on that origin, whichever URL it was installed from. Badging it
 *  with the workspace that happens to be on screen would hide every other
 *  workspace's unreads. A bare root context (`WORKSPACE_ID === null`, i.e. a
 *  legacy no-gateway engine or a direct engine-port page) is the opposite case:
 *  that origin serves exactly one workspace, its manifest keeps the bundled
 *  relative scope, and the control plane it would have to ask does not exist
 *  there. */
function badgesEveryWorkspace(): boolean {
  return WORKSPACE_ID !== null;
}

/** Unread notifications across every workspace on this origin EXCEPT this one.
 *
 *  Our own row is dropped rather than read back. For a second or two after an
 *  optimistic mark-read it still reports the pre-read count. Using it would let
 *  the icon disagree with the bell about the workspace on screen. */
export const otherWorkspacesUnread = computed(() =>
  peerWorkspaces.value
    .filter((w) => w.id !== WORKSPACE_ID)
    .reduce((sum, w) => sum + (w.unread_count ?? 0), 0),
);

/** The number BOTH badges show: this workspace's live unread count plus every
 *  other workspace behind the gateway.
 *
 *  One computed, two readers. `syncWorkspaceAppBadge` writes it onto the
 *  installed app's icon, and the brand mark renders it. Deriving the same total
 *  twice is exactly how the two would drift apart.
 *
 *  Our own half is ALWAYS the live `unreadCount`, so an optimistic mark-read
 *  drops both on the same tick. */
export const crossWorkspaceUnreadTotal = computed(
  () => unreadCount.value + (badgesEveryWorkspace() ? otherWorkspacesUnread.value : 0),
);

/** Re-read the OTHER workspaces' unread counts from the gateway and re-assert
 *  the icon badge. A no-op outside a gateway-served workspace context.
 *
 *  The counts come from the same control listing the in-app workspace switcher
 *  renders (`GET /~/api/v1/control/workspaces`), refreshed by the gateway's 2s
 *  supervise loop. A stopped workspace reports no count and contributes 0, the
 *  same "running workspaces only" rule the picker badge and the desktop dock
 *  badge already follow.
 *
 *  The rows are stored whole, but only the OTHERS are counted. Our own row is
 *  dropped, and {@link crossWorkspaceUnreadTotal} supplies this workspace's
 *  share from the live `unreadCount` instead. An optimistic mark-read therefore
 *  drops the icon on the same tick, and the icon can never disagree with the
 *  bell about the workspace on screen.
 *
 *  Best-effort, per `.claude/rules/frontend.md`'s telemetry carve-out: it runs
 *  on a timer and on resume rather than on user intent, a blip keeps the
 *  last-good total instead of flashing a wrong one, and the next refresh (or
 *  the next push, which carries a fresh aggregate) recovers on its own. A toast
 *  would be noise about a number the user is not looking at. */
export async function refreshOtherWorkspacesUnread(): Promise<void> {
  // NOT gated on Tauri, unlike the icon write below. The desktop client badges
  // its dock from Rust. Its PAGE still draws the brand badge and the menu's
  // notifications group, and both need these rows.
  if (IS_PICKER || !badgesEveryWorkspace()) return;
  const seq = ++refreshSeq;
  let rows: WorkspaceStatus[];
  try {
    rows = await listWorkspaces();
  } catch (e) {
    // Telemetry carve-out (`.claude/rules/frontend.md`): nobody asked for this
    // request, it is about a number on an icon the user may not even have
    // installed, and a toast would announce a gateway blip they cannot act on.
    // It self-recovers: the next resume, tick, or push re-establishes the total.
    console.warn('[app-badge] cross-workspace unread refresh failed; keeping the last total', e);
    return;
  }
  if (seq !== refreshSeq) return; // superseded while in flight (see refreshSeq)
  peerWorkspaces.value = rows;
  syncWorkspaceAppBadge();
}

/** Re-assert this install's app-icon badge: this workspace's `unreadCount` (the
 *  same single source the bell badge and the Unread tab project from, so the
 *  two can never show different numbers) PLUS every other workspace behind the
 *  gateway (see {@link badgesEveryWorkspace} for why the whole origin shares
 *  one icon, and {@link refreshOtherWorkspacesUnread} for where the other half
 *  comes from).
 *
 *  Deliberately UNCONDITIONAL (it writes even when our own count didn't move),
 *  because the icon badge is an **externally written surface**: iOS sets it from
 *  the push payload's top-level `app_badge` in its parent process without ever
 *  running the page, and the service worker's `push` handler sets it on
 *  Chrome/Android — both while this page is backgrounded or closed. The page is
 *  therefore the only actor that knows the CURRENT truth, and it has to be able
 *  to overwrite a value it never saw being written.
 *
 *  This is why the `unreadCount` effect in `store/effects.ts` cannot be the only
 *  writer: a computed whose recomputed value is equal does not notify its
 *  subscribers, so a reload landing the SAME count re-runs nothing. Read the
 *  notification on another device, come back to a resident iOS PWA, and the
 *  count goes 0 → 0 while the icon still carries the 1 the push wrote — bell 0,
 *  icon 1, forever. Every path that (re)establishes the unread truth calls this
 *  (`loadUnreadNotifications`, the mark-read paths, resume).
 *
 *  Context-gated at CALL time: the gateway picker sets the cross-workspace
 *  aggregate itself (`WorkspacePicker.tsx`), and the Tauri desktop app drives a
 *  native dock badge / tray title from the gateway total. */
export function syncWorkspaceAppBadge(): void {
  if (IS_PICKER || isTauri()) return;
  applyAppBadge(crossWorkspaceUnreadTotal.value);
}
