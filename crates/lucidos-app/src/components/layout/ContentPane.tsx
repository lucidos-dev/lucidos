import { useEffect, useRef } from 'preact/hooks';
import { activeMenuItem, panelOverlay, parseRepoPath, type PanelOverlay } from '../../store/store';
import { useScrollMemory, contentScrollKey } from '../../hooks/useScrollMemory';
import { lazyComponent } from '../../utils/lazyComponent';
import { forceIOSRepaint } from '../../utils/iosRepaint';
import { onPageResume } from '../../utils/pageResume';

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

/** The one view the iOS repaint below skips. The app-ui body is `overflow: hidden`
 *  with an iframe child, so it is not the scroll container that blanks, and the
 *  iframe composites itself out of our reach: the repaint buys nothing there. It
 *  also costs something. `forceIOSRepaint` writes a transform for one frame, which
 *  makes `.content-pane-body` the containing block for the pseudo-fullscreen app
 *  panel's `position: fixed` (`.app-ui-fullscreen`, rendered in-tree by
 *  AppUiInline), snapping a fullscreen app back to the pane's box for that frame.
 *  Reads the signal live rather than the render's `isAppUi`, because the resume
 *  subscription is mounted once and outlives every overlay change. */
function hostsAppUiIframe(): boolean {
  return panelOverlay.peek()?.type === 'app-ui';
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

  const isAppUi = overlay?.type === 'app-ui';

  const bodyRef = useRef<HTMLDivElement>(null);
  const viewKey = contentViewKey(active, overlay);
  // resetOnEmpty: this body hosts every view; without it, a stale scrollTop
  // from the prior view persists on the DOM and reappears when content grows.
  useScrollMemory(bodyRef, viewKey ? contentScrollKey(viewKey) : null, { resetOnEmpty: true });

  // iOS PWA paint loss (see utils/iosRepaint.ts for the mechanism).
  // `.content-pane-body` is an `overflow-y: auto` scroll container, so WKWebView
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
  // tried and REVERTED: `forceIOSRepaint`'s recovery nudge writes `scrollTop`, and
  // `useHideOnScroll` listens for scroll on this exact element
  // (`.mobile-swipe-pane .content-pane-body`), so each nudge moved the mobile
  // header and rewrote the `--mobile-header-offset` custom property on `:root`. As
  // a 5-attempt burst that came to ten header transforms plus five forced
  // synchronous layouts per view change, landing while the incoming view mounts its
  // lazy chunk and its data. The notification detail keys `viewKey` per
  // notification, so every prev/next chevron tap paid it, on a path already
  // optimized to drop a network round-trip for latency. Reported as lag opening
  // notifications. Do not reintroduce a navigation-triggered repaint here without
  // first making scroll consumers ignore the repaint nudge.
  useEffect(() => onPageResume(() => {
    if (hostsAppUiIframe()) return;
    forceIOSRepaint(bodyRef.current);
  }), []);

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
