import { useLayoutEffect, useState } from 'preact/hooks';
import { currentApp, appPseudoFullscreen, appRefreshKey, appsList } from '../../store/store';
import { appFullscreenHost, syncAppFullscreenHost } from '../../store/appFullscreenHost';
import { getAppFrameSrc, exitPseudoFullscreen, refreshAppUI } from '../../store/actions/apps';
import { usePanelRefresh } from '../../hooks/usePanelRefresh';
import { showPullTravel } from '../../store/panelRefresh';
import { ExitFullscreenIcon } from '../shared/icons';
import { viewportIsMobile } from '../../utils/viewport';
import { splitFrameSrc } from './iframeNav';
import { AppFrame, cacheBust } from './AppFrame';
import { EdgeSwipeZones } from '../layout/EdgeSwipeZones';
import type { AppReveal } from '../../store/types';
import { liveAppReveal, revealFuseMs } from './appFrameReveal';
import { FindBar } from '../shared/FindBar';
import { rerunFindOn } from '../../store/actions/find-bar';

/** Added to the open app's reveal fuse to cap how long a refresh waits for
 *  the new frame: the refresh's own debounce plus some slack. */
const REFRESH_SETTLE_SLACK_MS = 500;

/** Counts frame mounts. A refresh waits for a frame newer than the one on
 *  screen, whose own first load may still be pending. */
let framesMounted = 0;

/** App refreshes waiting for a frame mounted after they were asked. */
let frameLoadWaiters: Array<{ after: number; settle: () => void }> = [];

function settleFrameLoad(frame: number): void {
  const waiting = frameLoadWaiters;
  frameLoadWaiters = waiting.filter((w) => w.after >= frame);
  for (const w of waiting) if (w.after < frame) w.settle();
}

/** The app panel's refresh (the panel refresh contract): reload the frame and
 *  settle once the new one has loaded. preserveWip, because a refresh re-reads
 *  whatever the frame points at, WIP included. Apply landing and direct
 *  file-source edits call `refreshAppUI` with the default instead. */
function refreshOpenApp(reveal: AppReveal): Promise<void> {
  return new Promise((resolve) => {
    const fuse = setTimeout(settle, revealFuseMs(reveal) + REFRESH_SETTLE_SLACK_MS);
    function settle() {
      clearTimeout(fuse);
      resolve();
    }
    frameLoadWaiters.push({ after: framesMounted, settle });
    void refreshAppUI(undefined, { preserveWip: true });
  });
}

/** The Canvas pane's frame. Counts its mount, so a refresh waits for a frame
 *  newer than the one on screen, and drops the pull arrow when it goes. */
function ContentAppFrame({ src, reveal }: { src: string; reveal: AppReveal }) {
  const [frame] = useState(() => ++framesMounted);
  return (
    <AppFrame
      src={src}
      reveal={reveal}
      onRevealed={() => settleFrameLoad(frame)}
      onLoad={() => void rerunFindOn('content')}
      onUnmount={() => showPullTravel(0)}
    />
  );
}

export function AppUiInline({ layout }: { layout: 'desktop' | 'mobile' }) {
  const app = currentApp.value;
  const refreshKey = appRefreshKey.value;
  const isPseudo = appPseudoFullscreen.value;
  const reveal = app ? liveAppReveal(app, appsList.value) : 'on-load';
  // Skip mounting the iframe in the inactive dual-rendered layout — otherwise
  // every app open spawns two iframes loading the same id.
  const isActiveLayout = layout === (viewportIsMobile.value ? 'mobile' : 'desktop');
  usePanelRefresh(`app "${app?.name ?? ''}"`, app && isActiveLayout ? () => refreshOpenApp(reveal) : null);

  // Gate the layout effect on isActiveLayout so the inactive copy doesn't fight
  // the active one over the global attribute (its cleanup would clear what the
  // active copy just set when the inactive copy unmounts on viewport change).
  useLayoutEffect(() => {
    if (!isActiveLayout) return;
    document.documentElement.toggleAttribute('data-pseudo-fullscreen', isPseudo);
    return () => document.documentElement.removeAttribute('data-pseudo-fullscreen');
  }, [isPseudo, isActiveLayout]);

  // Keep `appFullscreenHost` (where the host's overlay layer renders) in step
  // with the DOM: while this panel is the natively fullscreen element, the
  // overlays have to be portaled INSIDE it, because a fullscreen element is
  // painted alone. `syncAppFullscreenHost` re-derives that from the document
  // rather than from anything captured here, so a replaced panel cannot leave a
  // stale answer behind (see store/appFullscreenHost.ts).
  //
  // No dep array: it re-asserts on every render as well as on the fullscreen
  // event, so a remount is covered by the render that caused it. It only ever
  // writes a CHANGED value, so a render that changes nothing does not churn the
  // overlay layer's portal target. Same isActiveLayout gate as above.
  useLayoutEffect(() => {
    if (isActiveLayout) syncAppFullscreenHost();
  });

  // The listeners and the on-unmount clear are keyed separately from the sync
  // above: they must NOT be torn down and rebuilt on every render, and the clear
  // must fire when this panel actually goes away rather than between two renders
  // that both have a fullscreen app (which would unmount and remount the whole
  // overlay layer, and any open modal with it).
  useLayoutEffect(() => {
    if (!isActiveLayout) return;
    document.addEventListener('fullscreenchange', syncAppFullscreenHost);
    document.addEventListener('webkitfullscreenchange', syncAppFullscreenHost);
    return () => {
      document.removeEventListener('fullscreenchange', syncAppFullscreenHost);
      document.removeEventListener('webkitfullscreenchange', syncAppFullscreenHost);
      appFullscreenHost.value = null;
    };
  }, [isActiveLayout]);

  if (!app) return null;
  if (!isActiveLayout) return null;

  const baseSrc = getAppFrameSrc();
  const frameSrc = (baseSrc && refreshKey > 0) ? cacheBust(baseSrc, refreshKey) : baseSrc;

  // The key carries BOTH the refresh counter and the document. Either one
  // changing mounts a fresh iframe with the new URL as its initial src (no
  // double-load). The document half is what an app switch rides: an isolated
  // frame denies `contentWindow.location`, and an iframe's first load adds no
  // history entry where mutating `src` would.
  return (
    <div
      data-role="app-ui-panel"
      class={`app-ui-inline content-view-full-bleed${isPseudo ? ' app-ui-fullscreen' : ''}`}
    >
      {isPseudo && (
        <>
          <button class="pseudo-fullscreen-exit icon-btn" onClick={exitPseudoFullscreen} aria-label="Exit fullscreen">
            <ExitFullscreenIcon />
          </button>
          {/* The swipe panes carry these strips too, but this overlay is
              position:fixed at var(--z-app-fullscreen) over the whole viewport and
              covers them, so an edge touch went straight into the app iframe
              and WebKit's native back gesture in the standalone iOS PWA claimed
              it: a swipe from the left edge left fullscreen. Re-hang them here
              so the host sees the touch and can preventDefault it. They do NOT
              bring the pane swipe back, which stays off while fullscreen. */}
          {layout === 'mobile' && <EdgeSwipeZones />}
        </>
      )}
      <FindBar surface="content" scope={`app:${app.id}`} placeholder="Find in app" />
      {frameSrc && (
        <ContentAppFrame key={`${refreshKey}:${splitFrameSrc(frameSrc).doc}`} src={frameSrc} reveal={reveal} />
      )}
      {/* Where the host's overlay layer renders while this panel is natively
          fullscreen (OverlayLayer portals into it, found by this marker). Always
          mounted, always empty: it has no vnode children, so the portal is the
          only writer, and `display: contents` keeps it out of the flex column
          and out of hit testing. Last child, so the overlays follow the iframe
          in paint order. */}
      <div class="app-overlay-layer" data-overlay-layer="" />
    </div>
  );
}
