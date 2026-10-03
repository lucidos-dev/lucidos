import { useRef, useLayoutEffect, useState, useEffect } from 'preact/hooks';
import { currentApp, appPseudoFullscreen, appRefreshKey, appsList, scaledDurationMs } from '../../store/store';
import { appFullscreenHost, syncAppFullscreenHost } from '../../store/appFullscreenHost';
import { getAppFrameSrc, exitPseudoFullscreen, refreshAppUI } from '../../store/actions/apps';
import { usePanelRefresh } from '../../hooks/usePanelRefresh';
import { showPullTravel } from '../../store/panelRefresh';
import { ExitFullscreenIcon } from '../shared/icons';
import { viewportIsMobile } from '../../utils/viewport';
import { useDelayedFlag, useLingeringFlag } from '../../hooks/useDelayedLoading';
import { setAppFrameHash, splitFrameSrc } from './iframeNav';
import { APP_FRAME_SANDBOX, APP_FRAME_ALLOW } from './appFrameSandbox';
import { EdgeSwipeZones } from '../layout/EdgeSwipeZones';
import { pushKeybindingsToFrame } from '../../store/actions/app-keybindings';
import { pushAppearanceToFrame } from '../../store/actions/app-appearance';
import { IframeTabExit } from '../shared/IframeTabExit';
import { APP_FRAME_READY_EVENT } from '../../store/actions/app-ready-bridge';
import type { AppReveal } from '../../store/types';
import { appFrameLoading, appFrameRevealed, liveAppReveal, revealFuseMs } from './appFrameReveal';

/** The load cover's CSS opacity transition at 1x (var(--duration-normal)). The
 *  cover lingers for this, scaled by the Animation speed slider, plus fixed
 *  slack, so it stays mounted until the fade finishes at any setting, the same
 *  way the LoadingFade component holds a clearing skeleton. */
const COVER_FADE_MS = 200;
const COVER_FADE_SLACK_MS = 50;

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

/** Append a cache-busting query param to a URL. */
function cacheBust(url: string, key: number): string {
  const u = new URL(url, window.location.origin);
  u.searchParams.set('_r', String(key));
  return u.toString();
}

/** The iframe element, isolated so its useState resets per remount.
 *
 *  Freezes the JSX `src` at mount so Preact never diffs it. A live element must
 *  not have its `src` mutated: that adds an entry to iOS Safari's joint session
 *  history (WebKit #9166). The PWA's edge-swipe-back gesture then surfaces a
 *  snapshot of a previous app state mid-swipe. An app switch remounts this
 *  component instead, and a first load adds no entry. */
function AppFrame({ src, reveal }: { src: string; reveal: AppReveal }) {
  const iframeRef = useRef<HTMLIFrameElement>(null);
  const [initialSrc] = useState(src);
  const lastSrcRef = useRef(initialSrc);

  // A frame with no document yet paints its base canvas, which WKWebView fills
  // WHITE: on a dark theme every app open flashed white before the app's own
  // stylesheet (and `/api/v1/sdk-prefs.js`, a second request, which is what
  // actually applies the theme) landed. Both gaps are inside a document the
  // host does not author, so the host hides the frame instead: an opaque
  // theme-coloured cover from mount, crossfaded out once the frame has
  // something to show. See AppUiInline.test.ts for why the cover is a sibling
  // element rather than an opacity on the iframe itself.
  //
  // An app whose manifest says `"reveal": "on-ready"` keeps the cover until it
  // calls `lucidos.ui.ready()`, so its first data fetch is hidden too. The
  // decision is in appFrameReveal.ts.
  const [loaded, setLoaded] = useState(false);
  const [ready, setReady] = useState(false);
  const [fused, setFused] = useState(false);
  const signals = { loaded, ready, fused };
  const revealed = appFrameRevealed(reveal, signals);
  const coverMounted = useLingeringFlag(!revealed, scaledDurationMs(COVER_FADE_MS) + COVER_FADE_SLACK_MS);
  // The load bar is delay-gated, so a fast open shows none. It shows opaque
  // the frame the gate lets it through, and fades only on the way out.
  const barShown = useDelayedFlag(appFrameLoading(reveal, signals));
  const barMounted = useLingeringFlag(barShown, scaledDurationMs(COVER_FADE_MS) + COVER_FADE_SLACK_MS);
  const [frame] = useState(() => ++framesMounted);

  // A pull the frame was posting ends with it, so the arrow comes down.
  useEffect(() => () => showPullTravel(0), []);

  // Layout phase, so the listener is on the element before the browser paints
  // and before any frame script can run.
  useLayoutEffect(() => {
    const iframe = iframeRef.current;
    if (!iframe) return;
    const onReady = () => setReady(true);
    iframe.addEventListener(APP_FRAME_READY_EVENT, onReady);
    return () => iframe.removeEventListener(APP_FRAME_READY_EVENT, onReady);
  }, []);

  useEffect(() => {
    if (revealed) {
      settleFrameLoad(frame);
      return;
    }
    const fuse = setTimeout(() => setFused(true), revealFuseMs(reveal));
    return () => clearTimeout(fuse);
  }, [revealed]);

  // A FRAGMENT change only. The caller keys this component on the document. A
  // change of document therefore remounts it, and the new element carries the
  // new URL as its `initialSrc`. An iframe's first load is a history REPLACE,
  // which is the property the old imperative `location.replace` existed to
  // keep.
  //
  // The mount has to do it now rather than by choice. An isolated app frame
  // denies `contentWindow.location`, and mutating `src` on a live element is
  // what adds the joint-history entry (WebKit #9166). Only a remount needs
  // nothing from the app, and needing nothing matters: the SDK is opt-in, so an
  // app may have loaded none of it.
  //
  // The fragment is the opposite case. Delivering it must NOT reload the app,
  // so it goes over the bridge and the app answers it. No cover is raised: the
  // app is on screen and stays there. An emptied fragment is not a target, so
  // it moves nobody and the reader keeps their place.
  useLayoutEffect(() => {
    const iframe = iframeRef.current;
    if (!iframe) return;
    if (lastSrcRef.current === src) return;
    const next = splitFrameSrc(src);
    if (next.fragment) setAppFrameHash(iframe, next.fragment);
    lastSrcRef.current = src;
  }, [src]);

  return (
    <>
      <iframe
        ref={iframeRef}
        data-role="app-ui-frame"
        class="app-ui-iframe"
        src={initialSrc}
        sandbox={APP_FRAME_SANDBOX}
        allow={APP_FRAME_ALLOW}
        onLoad={(e) => {
          setLoaded(true);
          pushKeybindingsToFrame(e.currentTarget as HTMLIFrameElement);
          pushAppearanceToFrame(e.currentTarget as HTMLIFrameElement);
        }}
      />
      <IframeTabExit />
      {coverMounted && (
        <div class={`app-ui-cover${revealed ? ' is-clearing' : ''}`} aria-hidden="true" />
      )}
      {barMounted && (
        <div
          class={`app-ui-load-bar${barShown ? '' : ' is-clearing'}`}
          role="progressbar"
          aria-label="Loading app"
        />
      )}
    </>
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
      class={`app-ui-inline${isPseudo ? ' app-ui-fullscreen' : ''}`}
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
      {frameSrc && (
        <AppFrame key={`${refreshKey}:${splitFrameSrc(frameSrc).doc}`} src={frameSrc} reveal={reveal} />
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
