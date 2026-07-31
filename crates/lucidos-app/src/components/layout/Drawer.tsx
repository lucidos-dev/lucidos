import { signal } from '@preact/signals';
import { useRef } from 'preact/hooks';
import { activeMenuItem, panelOverlay, pinnedApps, appsList, changes, appliedChanges } from '../../store/store';
import { switchMenuItem } from '../../store/actions/menu';
import { openUrl } from '../../store/actions/artifacts';
import { openAppById } from '../../store/actions/apps';
import { showToast } from '../../store/store';
import { errorDetail } from '../../utils/errorDetail';
import { isTauri } from '../../utils/platform';
import { isMobile } from '../../utils/viewport';
import { useHidePanelWebviewWhile } from '../../hooks/useHidePanelWebviewWhile';
import { Overlay } from '../shared/Overlay';
import type { MenuItem } from '../../store/types';

const menuItems: Array<{ id: MenuItem; label: string }> = [
  { id: 'files', label: 'Files' },
  { id: 'apps', label: 'Apps' },
  { id: 'plugins', label: 'Plugins' },
  { id: 'triggers', label: 'Triggers' },
];

export const drawerOpen = signal(false);
export const drawerClosing = signal(false);
/** Hamburger button that opened the drawer. Several `.hamburger-panel` buttons
 *  exist (a per-layout copy, plus one per mobile pane header); only the one the
 *  user actually pressed fires openDrawer(), so this captures the right element
 *  for the dismiss hook's anchor exemption. */
export const drawerAnchor = signal<HTMLElement | null>(null);

export type DrawerSide = 'left' | 'right';

/** Which edge the drawer slides out from. */
export const drawerSide = signal<DrawerSide>('left');

/** The drawer emerges from under the button that opened it: the mobile thread
 *  pane header keeps its hamburger at the row's trailing edge (mirroring the
 *  thread drawer toggle at the leading edge), and a panel sliding in from the
 *  far side of the screen would read as unrelated to the tap.
 *
 *  Desktop is always `left`: its single hamburger sits at the content pane's
 *  leading edge and the panel is positioned to emerge from the split divider,
 *  not from a viewport edge, so the anchor's absolute x says nothing useful
 *  there. Pure so the rule is testable without a DOM. */
export function drawerSideFor(anchorCenterX: number, viewportWidth: number, mobile: boolean): DrawerSide {
  if (!mobile) return 'left';
  return anchorCenterX > viewportWidth / 2 ? 'right' : 'left';
}

/** Open the drawer, resetting any stuck closing state */
export function openDrawer(anchor?: HTMLElement) {
  drawerClosing.value = false;
  drawerOpen.value = true;
  if (anchor) {
    drawerAnchor.value = anchor;
    const rect = anchor.getBoundingClientRect();
    drawerSide.value = drawerSideFor(rect.left + rect.width / 2, window.innerWidth, isMobile());
  }
}

/** Close the drawer; returns `false` when already closed/closing so the
 *  dismiss hook keeps the paired click un-swallowed — `drawerOpen` stays
 *  `true` for the 200ms slide-out animation, and without this signal the
 *  hook would eat a user's tap on a neighbor button (file-search, content
 *  actions, …) as if they meant to dismiss. */
export function closeDrawer(): boolean {
  if (!drawerOpen.value || drawerClosing.value) return false;
  drawerClosing.value = true;
  return true;
}

/** Immediately close the drawer without animation (e.g. pane switching). */
export function forceCloseDrawer() {
  drawerOpen.value = false;
  drawerClosing.value = false;
}

interface PinnedUi {
  appId: string;
  appName: string;
}

/** Render rows only when both Loadables are `loaded` — falling through to
 *  `[]` mid-load would look like "user unpinned" instead of "still loading". */
function resolvedPinnedUis(): PinnedUi[] {
  const pinned = pinnedApps.value;
  const loaded = appsList.value;
  if (pinned.status !== 'loaded' || loaded.status !== 'loaded') return [];
  if (pinned.data.length === 0) return [];

  const result: PinnedUi[] = [];
  for (const entry of pinned.data) {
    const app = loaded.data.find((s) => s.id === entry.app_id);
    if (app) result.push({ appId: app.id, appName: app.name });
  }
  return result;
}

export function Drawer() {
  const isOpen = drawerOpen.value;
  const pinned = resolvedPinnedUis();
  const drawerRef = useRef<HTMLDivElement>(null);

  useHidePanelWebviewWhile(isOpen);

  if (!isOpen) return null;

  // Badge count: only the `loaded` signal contributes a real number. While
  // the changes Loadable is not-loaded / loading / failed, hide the badge
  // entirely rather than render `0` (which would look like "nothing to
  // review" during a DB outage). Reuses the existing `changeCount > 0`
  // gate below — `null` falls through it cleanly.
  const changesLoadable = changes.value;
  const changeCount: number | null =
    changesLoadable.status === 'loaded' ? changesLoadable.data.length : null;

  // Only surface the Changes entry when the user actually has changes to see —
  // pending OR applied/reverted history. Most users never make changes to
  // Lucidos, so this keeps the drawer uncluttered for them. Keep it visible
  // while the Changes view is the active menu item so a user viewing it isn't
  // stranded when the last change clears.
  const appliedLoadable = appliedChanges.value;
  const hasPendingChanges = changeCount !== null && changeCount > 0;
  const hasAppliedChanges =
    appliedLoadable.status === 'loaded' && appliedLoadable.data.length > 0;
  const showChanges =
    hasPendingChanges || hasAppliedChanges || activeMenuItem.value === 'changes';

  return (
    // The `.drawer-backdrop` wrapper stays the caller's own (it dims the chat
    // pane and carries the slide-out `closing` class); <Overlay backdrop={false}>
    // renders the `.drawer` panel inside it and owns the dismiss/swallow/Escape
    // contract. Anchor is the hamburger that opened this drawer (stamped in
    // openDrawer), so re-clicking it routes through its own toggle. closeDrawer
    // returns false mid-animation so the dismiss hook stops eating neighbor taps.
    <div class={`drawer-backdrop ${drawerClosing.value ? 'closing' : ''}`}>
      <Overlay
        open
        onClose={closeDrawer}
        anchor={drawerAnchor.value}
        backdrop={false}
        panelClass={`drawer drawer-${drawerSide.value} ${drawerClosing.value ? 'closing' : ''}`}
        panelRef={drawerRef}
        panelProps={{
          onAnimationEnd: (e) => {
            if (drawerClosing.value && e.target === e.currentTarget) {
              drawerClosing.value = false;
              drawerOpen.value = false;
            }
          },
        }}
      >
        {/* Pinned app UIs first */}
        {pinned.map((p) => (
          <div
            key={`pin-${p.appId}`}
            class="drawer-item"
            onClick={() => {
              openAppById(p.appId).catch((err) => {
                showToast(`Failed to open app: ${errorDetail(err)}`, 'error');
              });
              closeDrawer();
            }}
          >
            {p.appName}
          </div>
        ))}

        {/* Menu items */}
        {menuItems.map((item) => (
          <div
            key={item.id}
            class={`drawer-item ${activeMenuItem.value === item.id ? 'active' : ''}`}
            onClick={() => {
              switchMenuItem(item.id);
              closeDrawer();
            }}
          >
            {item.label}
          </div>
        ))}

        {isTauri() && (
          <div
            class={`drawer-item ${panelOverlay.value?.type === 'url-preview' ? 'active' : ''}`}
            onClick={() => {
              if (panelOverlay.value?.type !== 'url-preview') {
                openUrl('https://www.google.com');
              }
              closeDrawer();
            }}
          >
            Browser
          </div>
        )}

        {showChanges && (
          <div
            class={`drawer-item ${activeMenuItem.value === 'changes' ? 'active' : ''}`}
            onClick={() => {
              switchMenuItem('changes');
              closeDrawer();
            }}
          >
            Changes
            {changeCount !== null && changeCount > 0 && (
              <span class="drawer-badge">{changeCount > 99 ? '99+' : changeCount}</span>
            )}
          </div>
        )}

        <div
          class={`drawer-item ${activeMenuItem.value === 'settings' ? 'active' : ''}`}
          onClick={() => {
            switchMenuItem('settings');
            closeDrawer();
          }}
        >
          Settings
        </div>
      </Overlay>
    </div>
  );
}
