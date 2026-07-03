import { useRef } from 'preact/hooks';
import { activeMenuItem, panelOverlay, parseRepoPath, type PanelOverlay } from '../../store/store';
import { useScrollMemory, contentScrollKey } from '../../hooks/useScrollMemory';
import { lazyComponent } from '../../utils/lazyComponent';

const FilesView = lazyComponent(() => import('../files/FilesView').then(m => m.FilesView));
const AppsView = lazyComponent(() => import('../apps/AppsView').then(m => m.AppsView));
const PluginsView = lazyComponent(() => import('../plugins/PluginsView').then(m => m.PluginsView));
const TriggersView = lazyComponent(() => import('../triggers/TriggersView').then(m => m.TriggersView));
const SettingsView = lazyComponent(() => import('../settings/SettingsView').then(m => m.SettingsView));
const ChangesView = lazyComponent(() => import('../changes/ChangesView').then(m => m.ChangesView));
const NotificationsView = lazyComponent(() => import('../notifications/NotificationsView').then(m => m.NotificationsView));
const FilePreviewInline = lazyComponent(() => import('../files/FilePreviewInline').then(m => m.FilePreviewInline));
const RepoFilePreviewWithSidebar = lazyComponent(() => import('../files/RepoFilePreview').then(m => m.RepoFilePreviewWithSidebar));
const UrlPreviewInline = lazyComponent(() => import('../files/UrlPreviewInline').then(m => m.UrlPreviewInline));
const AppUiInline = lazyComponent(() => import('../apps/AppUiInline').then(m => m.AppUiInline));
const InlineForm = lazyComponent(() => import('./InlineForm').then(m => m.InlineForm));
const NotificationDetailInline = lazyComponent(() => import('../notifications/NotificationDetailInline').then(m => m.NotificationDetailInline));

// Per-overlay scoping so scroll resets when switching between, say, two file
// previews. Returns null when there's nothing to key on, so the hook skips.
function contentViewKey(active: string | null, overlay: PanelOverlay): string | null {
  if (overlay) {
    if (overlay.type === 'file-preview') return `file:${overlay.path}`;
    if (overlay.type === 'url-preview') return `url:${overlay.url}`;
    if (overlay.type === 'notification-detail') return `notification:${overlay.notification.id}`;
    return overlay.type;
  }
  return active;
}

/** Rendered twice — once by SplitLayout, once by MobileSwipeContainer. The
 *  `layout` prop lets dual-render-aware children skip work in the inactive
 *  copy. */
export function ContentPane({ layout }: { layout: 'desktop' | 'mobile' }) {
  const active = activeMenuItem.value;
  const overlay = panelOverlay.value;

  const isAppUi = overlay?.type === 'app-ui';

  const bodyRef = useRef<HTMLDivElement>(null);
  const viewKey = contentViewKey(active, overlay);
  // resetOnEmpty: this body hosts every view; without it, a stale scrollTop
  // from the prior view persists on the DOM and reappears when content grows.
  useScrollMemory(bodyRef, viewKey ? contentScrollKey(viewKey) : null, { resetOnEmpty: true });

  return (
    <div class="content-pane">
      {/* Focusable scroll region (mirrors `.thread-content`): when the focused-pane
          marker moves onto the content pane, `reconcilePaneFocus` lands DOM focus here so
          native Arrow/Page/Home/End keys scroll it. `tabIndex=0` + role/label keep
          the scrollable region keyboard-reachable and discoverable. */}
      <div
        class={`content-pane-body${isAppUi ? ' has-app-ui' : ''}`}
        ref={bodyRef}
        tabIndex={0}
        role="region"
        aria-label="Content pane"
      >

        {overlay?.type === 'form' && <InlineForm />}
        {overlay?.type === 'app-ui' && <AppUiInline layout={layout} />}
        {overlay?.type === 'file-preview' && (() => {
          const repo = parseRepoPath(overlay.path);
          return repo
            ? <RepoFilePreviewWithSidebar repoId={repo.repoId} mode={repo.mode} path={repo.path} changeId={repo.changeId} layout={layout} />
            : <FilePreviewInline path={overlay.path} layout={layout} />;
        })()}
        {overlay?.type === 'url-preview' && <UrlPreviewInline url={overlay.url} layout={layout} />}
        {overlay?.type === 'notification-detail' && <NotificationDetailInline />}
        {!overlay && (
          <>
            {active === 'files' && <FilesView />}
            {active === 'apps' && <AppsView />}
            {active === 'plugins' && <PluginsView />}
            {active === 'triggers' && <TriggersView />}
            {active === 'settings' && <SettingsView />}
            {active === 'changes' && <ChangesView />}
            {active === 'notifications' && <NotificationsView />}
          </>
        )}
      </div>
    </div>
  );
}
