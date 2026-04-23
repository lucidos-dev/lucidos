import { signal } from '@preact/signals';
import { useEffect } from 'preact/hooks';
import { activeMenuItem, panelOverlay, pinnedApps, appsList, changes } from '../../store/store';
import { switchMenuItem } from '../../store/actions/menu';
import { openUrl } from '../../store/actions/artifacts';
import { openAppById } from '../../store/actions/apps';
import { isTauri } from '../../utils/platform';
import { hidePanelWebview, showPanelWebview } from '../../utils/tauri';
import type { MenuItem } from '../../store/types';

const menuItems: Array<{ id: MenuItem; label: string }> = [
  { id: 'files', label: 'Files' },
  { id: 'apps', label: 'Apps' },
  { id: 'triggers', label: 'Triggers' },
];

export const drawerOpen = signal(false);
export const drawerClosing = signal(false);

/** Open the drawer, resetting any stuck closing state */
export function openDrawer() {
  drawerClosing.value = false;
  drawerOpen.value = true;
}

/** Close the drawer with a slide-out animation */
export function closeDrawer() {
  if (!drawerOpen.value || drawerClosing.value) return;
  drawerClosing.value = true;
}

/** Immediately close the drawer without animation (e.g. pane switching). */
export function forceCloseDrawer() {
  drawerOpen.value = false;
  drawerClosing.value = false;
}

/** Resolve pinned app UI entries to displayable objects */
function resolvedPinnedUis() {
  const entries = pinnedApps.value;
  const loaded = appsList.value;
  if (entries.length === 0 || loaded.status !== 'loaded') return [];

  return entries
    .map((entry) => {
      const app = loaded.data.find((s) => s.id === entry.app_id);
      if (!app) return null;
      return { appId: app.id, appName: app.name };
    })
    .filter((x) => x != null);
}

export function Drawer() {
  const isOpen = drawerOpen.value;
  const pinned = resolvedPinnedUis();

  useEffect(() => {
    if (!isOpen || !isTauri()) return;
    hidePanelWebview();
    return () => showPanelWebview();
  }, [isOpen]);

  // Desktop backdrop only covers the panel area — catch clicks on the chat pane too
  useEffect(() => {
    if (!isOpen) return;
    function handleMouseDown(e: MouseEvent) {
      const target = e.target as HTMLElement;
      if (target.closest('.drawer') || target.closest('.hamburger-panel') || target.closest('.thread-toggle')) return;
      closeDrawer();
    }
    document.addEventListener('mousedown', handleMouseDown);
    return () => document.removeEventListener('mousedown', handleMouseDown);
  }, [isOpen]);

  if (!isOpen) return null;

  const changeCount = changes.value.length;

  return (
    <div
      class={`drawer-backdrop ${drawerClosing.value ? 'closing' : ''}`}
      onClick={() => closeDrawer()}
    >
      <nav
        class={`drawer ${drawerClosing.value ? 'closing' : ''}`}
        onClick={(e) => e.stopPropagation()}
        onAnimationEnd={(e) => {
          if (drawerClosing.value && e.target === e.currentTarget) {
            drawerClosing.value = false;
            drawerOpen.value = false;
          }
        }}
      >
        {/* Pinned app UIs first */}
        {pinned.map((p) => (
          <div
            key={`pin-${p!.appId}`}
            class="drawer-item"
            onClick={() => {
              openAppById(p!.appId);
              closeDrawer();
            }}
          >
            {p!.appName}
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

        <div
          class={`drawer-item ${activeMenuItem.value === 'changes' ? 'active' : ''}`}
          onClick={() => {
            switchMenuItem('changes');
            closeDrawer();
          }}
        >
          Changes
          {changeCount > 0 && <span class="drawer-badge">{changeCount > 99 ? '99+' : changeCount}</span>}
        </div>

        <div
          class={`drawer-item ${activeMenuItem.value === 'settings' ? 'active' : ''}`}
          onClick={() => {
            switchMenuItem('settings');
            closeDrawer();
          }}
        >
          Settings
        </div>
      </nav>
    </div>
  );
}
