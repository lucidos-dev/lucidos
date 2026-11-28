import { useRef } from 'preact/hooks';
import { activeMenuItem, panelOverlay, pinnedApps, appsList, changes, appliedChanges, actionableChangeCount } from '../../store/store';
import { switchMenuItem } from '../../store/actions/menu';
import { openUrl } from '../../store/actions/artifacts';
import { revealContentPane } from '../../store/actions/pane';
import { inAppBrowserAvailable } from '../../store/actions/preferences';
import { openAppById } from '../../store/actions/apps';
import { showToast } from '../../store/store';
import { errorDetail } from '../../utils/errorDetail';
import { useHidePanelWebviewWhile } from '../../hooks/useHidePanelWebviewWhile';
import { Overlay } from '../shared/Overlay';
import { SystemAttentionBadge } from '../shared/SystemAttentionBadge';
import { systemAttentionBadge } from '../../store/systemAttentionBadge';
import { MENU_ITEM_LABELS, type MenuItem } from '../../store/types';
import { closeDrawer, drawerAnchor, drawerClosing, drawerOpen, drawerSide } from './drawerState';
import { GlyphBadge } from '../shared/GlyphBadge';
import { AppIcon } from '../shared/AppIcon';
import { CategoryIcon } from '../shared/CategoryIcon';

const menuItems: MenuItem[] = ['files', 'apps', 'plugins', 'triggers'];

interface PinnedUi {
  appId: string;
  appName: string;
  appIcon?: string;
}

function resolvedPinnedUis(): PinnedUi[] {
  const pinned = pinnedApps.value;
  const loaded = appsList.value;
  if (pinned.status !== 'loaded' || loaded.status !== 'loaded') return [];
  if (pinned.data.length === 0) return [];

  const result: PinnedUi[] = [];
  for (const entry of pinned.data) {
    const app = loaded.data.find((s) => s.id === entry.app_id);
    if (app) result.push({ appId: app.id, appName: app.name, appIcon: app.icon });
  }
  return result;
}

function DrawerGlyph({ category }: { category: string }) {
  return (
    <span class="drawer-item-glyph" aria-hidden="true">
      <CategoryIcon category={category} />
    </span>
  );
}

export function Drawer() {
  const isOpen = drawerOpen.value;
  const pinned = resolvedPinnedUis();
  const drawerRef = useRef<HTMLDivElement>(null);

  useHidePanelWebviewWhile(isOpen);

  if (!isOpen) return null;

  const changeBadge = actionableChangeCount.value;

  const changesLoadable = changes.value;
  const appliedLoadable = appliedChanges.value;
  const hasPendingChanges = changesLoadable.status === 'loaded' && changesLoadable.data.length > 0;
  const hasAppliedChanges =
    appliedLoadable.status === 'loaded' && appliedLoadable.data.length > 0;
  const showChanges =
    hasPendingChanges || hasAppliedChanges || activeMenuItem.value === 'changes';

  return (
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
        {pinned.map((p) => (
          <div
            key={`pin-${p.appId}`}
            class="drawer-item"
            onClick={() => {
              openAppById(p.appId).catch((err) => {
                showToast(`Failed to open app "${p.appName}": ${errorDetail(err)}`, 'error');
              });
              closeDrawer();
            }}
          >
            <AppIcon appId={p.appId} name={p.appName} icon={p.appIcon} />
            {p.appName}
          </div>
        ))}
        {pinned.length > 0 && <div class="drawer-menu-divider" aria-hidden="true" />}

        {menuItems.map((item) => (
          <div
            key={item}
            class={`drawer-item ${activeMenuItem.value === item ? 'active' : ''}`}
            onClick={() => {
              switchMenuItem(item);
              closeDrawer();
            }}
          >
            <DrawerGlyph category={item} />
            {MENU_ITEM_LABELS[item]}
          </div>
        ))}

        {inAppBrowserAvailable() && (
          <div
            class={`drawer-item ${panelOverlay.value?.type === 'url-preview' ? 'active' : ''}`}
            onClick={() => {
              if (panelOverlay.value?.type !== 'url-preview') {
                openUrl('https://www.google.com');
              } else {
                revealContentPane();
              }
              closeDrawer();
            }}
          >
            <DrawerGlyph category="web" />
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
            <DrawerGlyph category="changes" />
            <span class="drawer-item-label">{MENU_ITEM_LABELS.changes}</span>
            {changeBadge !== null && changeBadge > 0 && (
              <GlyphBadge class="drawer-badge">{changeBadge > 99 ? '99+' : changeBadge}</GlyphBadge>
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
          <DrawerGlyph category="settings" />
          {MENU_ITEM_LABELS.settings}
          <SystemAttentionBadge placement="inline" label={systemAttentionBadge()} />
        </div>
      </Overlay>
    </div>
  );
}
