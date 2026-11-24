import { FindBar } from '../shared/FindBar';
import { fileFindScope } from '../../store/actions/find-request';
import { useEffect, useLayoutEffect, useRef } from 'preact/hooks';
import { activeMenuItem, appPseudoFullscreen, panelOverlay, settingsSubview, notificationDetailPending, parseRepoPath } from '../../store/store';
import { nativeFullscreenElement } from '../../store/appFullscreenHost';
import { contentViewKey } from './contentViewKey';
import { createContentLandingClaim } from './contentLanding';
import { reportNavigation } from '../../utils/navigationMarks';
import { useScrollMemory } from '../../hooks/useScrollMemory';
import { contentScrollKey } from '../../store/savedScroll';
import { useDelayedFlag } from '../../hooks/useDelayedLoading';
import { SkeletonProvider } from '../shared/Skeleton';
import { NavigationCover } from '../shared/NavigationCover';
import { FilePreviewPath } from '../files/FilePreviewPath';
import { FilePreviewContextMenu } from '../files/FilePreviewContextMenu';
import { NotificationsView } from '../notifications/NotificationsView';
import { NotificationDetailInline } from '../notifications/NotificationDetailInline';
import { SettingsHome } from '../settings/SettingsHome';
import { lazyComponent } from '../../utils/lazyComponent';
import { prefetchWhenIdle } from '../../utils/idlePrefetch';
import { forceWebKitRepaint } from '../../utils/webkitRepaint';
import { onPageResume } from '../../utils/pageResume';
import { trackPullToRefresh } from '@lucidos/pull-to-refresh';
import { runPanelRefresh, showPullTravel } from '../../store/panelRefresh';
import { PullRefreshAffordance } from './PullRefreshAffordance';
import { PreviewPathContext } from '../files/previewViewState';

const FilesView = lazyComponent(() => import('../files/FilesView').then(m => m.FilesView));
const AppsView = lazyComponent(() => import('../apps/AppsView').then(m => m.AppsView));
const PluginsView = lazyComponent(() => import('../plugins/PluginsView').then(m => m.PluginsView));
const TriggersView = lazyComponent(() => import('../triggers/TriggersView').then(m => m.TriggersView));
const SettingsView = lazyComponent(() => import('../settings/SettingsView').then(m => m.SettingsView));
const ChangesView = lazyComponent(() => import('../changes/ChangesView').then(m => m.ChangesView));
const FilePreviewInline = lazyComponent(() => import('../files/FilePreviewInline').then(m => m.FilePreviewInline));
const RepoFilePreviewWithSidebar = lazyComponent(() => import('../files/RepoFilePreview').then(m => m.RepoFilePreviewWithSidebar));
const UrlPreviewInline = lazyComponent(() => import('../files/UrlPreviewInline').then(m => m.UrlPreviewInline));
const AppUiInline = lazyComponent(() => import('../apps/AppUiInline').then(m => m.AppUiInline));
const InlineForm = lazyComponent(() => import('./InlineForm').then(m => m.InlineForm));
const PluginDetailPage = lazyComponent(() => import('../plugins/PluginDetailPage').then(m => m.PluginDetailPage));

for (const view of [
  FilesView, AppsView, PluginsView, TriggersView, SettingsView, ChangesView,
  FilePreviewInline, RepoFilePreviewWithSidebar, UrlPreviewInline, AppUiInline, InlineForm,
  PluginDetailPage,
]) {
  prefetchWhenIdle(view);
}

function fullscreenCoversThePane(): boolean {
  return appPseudoFullscreen.peek() || nativeFullscreenElement() !== null;
}

export function ContentPane({ layout }: { layout: 'desktop' | 'mobile' }) {
  const active = activeMenuItem.value;
  const overlay = panelOverlay.value;
  const subview = settingsSubview.value;

  const isAppUi = overlay?.type === 'app-ui';

  const showPendingNotification =
    useDelayedFlag(notificationDetailPending.value !== null) && !overlay;

  const bodyRef = useRef<HTMLDivElement>(null);
  const viewKey = contentViewKey(active, overlay, subview);
  const landingClaimRef = useRef(createContentLandingClaim());
  useScrollMemory(bodyRef, viewKey ? contentScrollKey(viewKey) : null, {
    resetOnEmpty: true,
    shouldRestore: () => !landingClaimRef.current(viewKey, bodyRef.current),
  });

  const reportedKeyRef = useRef(viewKey);
  useLayoutEffect(() => {
    if (reportedKeyRef.current === viewKey) return;
    reportedKeyRef.current = viewKey;
    reportNavigation('content-view', viewKey);
  }, [viewKey]);

  useEffect(() => onPageResume(() => {
    if (fullscreenCoversThePane()) return;
    forceWebKitRepaint(bodyRef.current);
  }), []);

  useEffect(() => {
    const body = bodyRef.current;
    if (!body) return;
    return trackPullToRefresh(body, {
      onPull: showPullTravel,
      onRefresh: () => void runPanelRefresh(),
    });
  }, []);

  return (
    <div class="content-pane">
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
          return (
            <div class="file-preview-frame content-view-full-bleed">
              <FilePreviewPath path={overlay.path} />
              <FindBar surface="content" scope={fileFindScope(overlay.path)} placeholder="Find in file" />
              <FilePreviewContextMenu path={overlay.path} layout={layout}>
                <PreviewPathContext.Provider value={overlay.path}>
                  {repo
                    ? <RepoFilePreviewWithSidebar locator={repo} layout={layout} />
                    : <FilePreviewInline path={overlay.path} layout={layout} />}
                </PreviewPathContext.Provider>
              </FilePreviewContextMenu>
            </div>
          );
        })()}
        {overlay?.type === 'url-preview' && <UrlPreviewInline url={overlay.url} layout={layout} />}
        {overlay?.type === 'notification-detail' && <NotificationDetailInline />}
        {overlay?.type === 'plugin-detail' && (
          <PluginDetailPage marketplaceId={overlay.marketplaceId} pluginId={overlay.pluginId} name={overlay.name} />
        )}
        {showPendingNotification && (
          <SkeletonProvider><NotificationDetailInline /></SkeletonProvider>
        )}
        {!overlay && (
          <>
            {active === 'files' && <FilesView />}
            {active === 'apps' && <AppsView />}
            {active === 'plugins' && <PluginsView />}
            {active === 'triggers' && <TriggersView />}
            {active === 'settings' && (subview === 'main' ? <SettingsHome /> : <SettingsView />)}
            {active === 'changes' && <ChangesView />}
            {active === 'notifications' && <NotificationsView />}
          </>
        )}
      </div>
      <NavigationCover viewKey={viewKey} />
      <PullRefreshAffordance />
    </div>
  );
}
