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
// `lazyComponent` renders NOTHING until its chunk lands, so what the user must
// see at once is eager. The two notification views are reached from the bell
// in every header and from an OS push tap. Behind a chunk, its fetch swallowed
// the inbox toolbar and both skeletons. The pair adds under 2 KB gzipped.
// `SettingsHome` is eager for the same reason. The other views stay split, and
// are prefetched when idle below.
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

// A lazy view draws nothing until its chunk lands, not even its own skeleton,
// and a rebuild renames every chunk. Every view here is one tap away, so they
// load once the app is idle (ADR 0288) and a first open finds them in memory.
for (const view of [
  FilesView, AppsView, PluginsView, TriggersView, SettingsView, ChangesView,
  FilePreviewInline, RepoFilePreviewWithSidebar, UrlPreviewInline, AppUiInline, InlineForm,
]) {
  prefetchWhenIdle(view);
}

/** The one state the WebKit repaint below skips: something is painted FULLSCREEN
 *  over this pane. Not merely an app panel being open.
 *
 *  `forceWebKitRepaint` writes a transform for one frame, which makes
 *  `.content-pane-body` the containing block for a `position: fixed` descendant.
 *  The pseudo-fullscreen panel (`.app-ui-fullscreen`, rendered in-tree by
 *  AppUiInline) is one, so a repaint would snap it back to the pane's box for
 *  that frame. A natively fullscreen element is painted alone, so a repaint of
 *  the pane under it buys nothing and risks the same capture.
 *
 *  Native is asked of the DOM rather than of `appFullscreenHost`, which answers
 *  null when the element is not an app panel of ours. An app that fullscreens
 *  its own content makes the IFRAME that element, and the pane is still covered.
 *
 *  Read live rather than at render, because the resume subscription is mounted
 *  once and outlives every overlay change. Why this is not every app-ui overlay:
 *  docs/plans/2026-09-17-a-deep-link-repaints-what-it-landed.md. */
function fullscreenCoversThePane(): boolean {
  return appPseudoFullscreen.peek() || nativeFullscreenElement() !== null;
}

/** Mounted ONCE, by whichever layout `App` renders for the current viewport
 *  (SplitLayout on desktop, MobileSwipeContainer on mobile). It used to be
 *  mounted by both at the same time; the `layout` prop dates from that era and is
 *  still forwarded so the children that mount something heavy (the app-ui iframe,
 *  the file / url / repo previews) can skip the inactive copy. Single-mount is
 *  load-bearing for the resume subscription below: a second live copy would
 *  register a second repaint callback on the same wake. */
export function ContentPane({ layout }: { layout: 'desktop' | 'mobile' }) {
  const active = activeMenuItem.value;
  const overlay = panelOverlay.value;
  // Part of the view key, so a Settings sub-section switch is a navigation like
  // any other rather than a swap the pane never notices.
  const subview = settingsSubview.value;

  const isAppUi = overlay?.type === 'app-ui';

  // A notification detail is being fetched and hasn't landed yet. Delay-gated
  // (`.claude/rules/frontend.md`: never render a loading indicator immediately)
  // and dropped as soon as the real overlay exists, so the skeleton and the
  // content can't both be on screen.
  //
  // `!overlay` rather than "replace whatever is showing": the fetch only happens
  // when neither notification list holds the row, which in practice is the cold
  // push-tap deep link (a warm page has loaded the unread set for its bell
  // badge, so the tapped row resolves from memory). A cold boot has no overlay
  // yet, so this is the case that matters, and it keeps the skeleton from
  // tearing down a live app-ui iframe for the length of a fetch.
  const showPendingNotification =
    useDelayedFlag(notificationDetailPending.value !== null) && !overlay;

  const bodyRef = useRef<HTMLDivElement>(null);
  const viewKey = contentViewKey(active, overlay, subview);
  const landingClaimRef = useRef(createContentLandingClaim());
  // resetOnEmpty: this body hosts every view; without it, a stale scrollTop
  // from the prior view persists on the DOM and reappears when content grows.
  // A landing on a row owns the open, so neither a restore nor the reset may
  // scroll the reader away from it.
  useScrollMemory(bodyRef, viewKey ? contentScrollKey(viewKey) : null, {
    resetOnEmpty: true,
    shouldRestore: () => !landingClaimRef.current(viewKey, bodyRef.current),
  });

  // The content half of the navigation mark: a view swap is a navigation
  // whether or not anything arrives to be covered. The first render is not one.
  const reportedKeyRef = useRef(viewKey);
  useLayoutEffect(() => {
    if (reportedKeyRef.current === viewKey) return;
    reportedKeyRef.current = viewKey;
    reportNavigation('content-view', viewKey);
  }, [viewKey]);

  // WebKit paint loss (see utils/webkitRepaint.ts for the mechanism), reported
  // first on the iOS PWA and since on the packaged desktop app.
  // `.content-pane-body` is a scroll container, so WKWebView
  // gives it its own compositing layer, and a backgrounded PWA (the phone locked)
  // leaves that layer frozen on a stale-or-empty backing texture: the panel is
  // fully rendered and laid out in the DOM, and nothing is on screen. Same blank
  // that was root-caused for `.thread-content` in the `ios-pwa-blackout`
  // investigation; the repaint hardening that closed it landed on the thread body
  // only, so this container was left as the surviving half of the same bug.
  //
  // Waking changes no signal (same panel, same data), so no render produces DOM
  // changes and only an explicit repaint can un-blank the layer. `onPageResume` is
  // the shared wake signal, covering pageshow / focus as well as visibilitychange
  // (iOS often restores a PWA through pageshow alone), so one wake already gets
  // three superseding attempts at it.
  //
  // ON RESUME ONLY, deliberately. A per-view repaint on every panel switch was
  // tried and REVERTED: `forceWebKitRepaint`'s recovery nudge writes `scrollTop`, and
  // `useHideOnScroll` listens for scroll on this exact element
  // (`.mobile-swipe-pane .content-pane-body`), so each nudge moved the mobile
  // header and rewrote the `--mobile-header-offset` custom property on `:root`. As
  // a 5-attempt burst that came to ten header transforms plus five forced
  // synchronous layouts per view change, landing while the incoming view mounts its
  // lazy chunk and its data. The notification detail keys `viewKey` per
  // notification, so every prev/next chevron tap paid it, on a path already
  // optimized to drop a network round-trip for latency. Reported as lag opening
  // notifications.
  //
  // That mechanism is gone as of 2026-08-03: `useHideOnScroll` now skips scroll
  // events inside the nudge window (`isRepaintNudging`), the offset is written on
  // its two consumer elements instead of `:root`, and it feeds `transform` rather
  // than `top`, so there is no header move and no forced layout left to pay. The
  // old precondition on reintroducing a navigation repaint is therefore satisfied.
  // It stays resume-only anyway, for the reason that outlived the regression: a
  // wake fires visibilitychange + pageshow + focus, so the resume path already
  // gets three superseding attempts, and a navigation repaint buys nothing a
  // switch-triggered render does not already cover.
  useEffect(() => onPageResume(() => {
    if (fullscreenCoversThePane()) return;
    forceWebKitRepaint(bodyRef.current);
  }), []);

  // Pull to refresh on whatever panel is open. A panel with nothing to refresh
  // registers nothing, and the runner and the affordance then do nothing.
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
          // The whole parsed locator, not four props teased out of it: it
          // carries a per-mode qualifier (the diff's change id, the file's git
          // ref) that the preview needs, and passing it whole is what keeps the
          // two mutually-exclusive halves from having to be reassembled here.
          const repo = parseRepoPath(overlay.path);
          // The path row is chrome over BOTH halves, so it is added here rather
          // than inside either preview: one row, whatever renders under it, and
          // no way for the two to drift. The column around them is what keeps
          // the preview's own box the size it was as a direct child of the pane
          // body (see `.file-preview-frame`).
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
        {/* A notification the page doesn't already hold is being fetched (the
            cold push-tap deep link; every other open resolves from memory and
            never gets here). The pane was revealed on the tap, so this fills it
            with the detail's own skeleton rather than an empty panel.

            The skeleton paints on the tap because the component is eager, per
            the import block at the top. Behind a lazy chunk it could not: the
            chunk fetch and the notification fetch raced, and the loser was the
            thing standing in for the other. */}
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
      {/* Outside `.content-pane-body`, so it covers the pane's viewport rather
          than scrolling away with the body's content, and so a remembered
          scrollTop being restored underneath it stays hidden. */}
      <NavigationCover viewKey={viewKey} />
      <PullRefreshAffordance />
    </div>
  );
}
