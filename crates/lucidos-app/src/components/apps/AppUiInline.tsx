import { useRef, useLayoutEffect, useState, useEffect } from 'preact/hooks';
import { currentApp, appPseudoFullscreen, appRefreshKey, showToast } from '../../store/store';
import { appFullscreenHost, syncAppFullscreenHost } from '../../store/appFullscreenHost';
import { getAppFrameSrc, exitPseudoFullscreen } from '../../store/actions/apps';
import { ExitFullscreenIcon } from '../shared/icons';
import { viewportIsMobile } from '../../utils/viewport';
import { useLingeringFlag } from '../../hooks/useDelayedLoading';
import { navigateAppIframe } from './iframeNav';
import { EdgeSwipeZones } from '../layout/EdgeSwipeZones';

/** How long the load cover lingers, fading out, after the frame's `load`. A
 *  little longer than its CSS opacity transition (var(--duration-normal) =
 *  200ms) so it stays mounted until the fade finishes, the same way the
 *  LoadingFade component holds a clearing skeleton. */
const COVER_FADE_MS = 250;

/** Reveal fuse for a frame whose `load` never arrives (a hung request). A pane
 *  covered forever is worse than whatever the frame managed to paint. */
const COVER_MAX_MS = 3000;

/** Append a cache-busting query param to a URL. */
function cacheBust(url: string, key: number): string {
  const u = new URL(url, window.location.origin);
  u.searchParams.set('_r', String(key));
  return u.toString();
}

/** The iframe element, isolated so its useState resets per refresh remount.
 *
 *  Freezes the JSX `src` at mount so Preact never diffs it. App switches must
 *  not let the renderer mutate `src` — that adds an entry to iOS Safari's
 *  joint session history (WebKit #9166), and the edge-swipe-back gesture in
 *  the PWA then surfaces a snapshot of a previous app state mid-swipe. App
 *  switches go through navigateAppIframe (location.replace) instead. */
function AppFrame({ src }: { src: string }) {
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
  const [loaded, setLoaded] = useState(false);
  const coverMounted = useLingeringFlag(!loaded, COVER_FADE_MS);

  useEffect(() => {
    if (loaded) return;
    const fuse = setTimeout(() => setLoaded(true), COVER_MAX_MS);
    return () => clearTimeout(fuse);
  }, [loaded]);

  useLayoutEffect(() => {
    const iframe = iframeRef.current;
    if (!iframe) return;
    if (lastSrcRef.current === src) return;
    // Skip lastSrcRef update on failure so the next render retries against a
    // freshly-mounted iframe rather than thinking the URL is already in place.
    // Stable key dedups the toast across rapid app switches that re-fire the
    // effect against a still-detaching iframe — one error sticks instead of N.
    if (!navigateAppIframe(iframe, src)) {
      showToast('Failed to navigate app frame: iframe has no browsing context', 'error', { key: 'app-iframe-nav-failed' });
      return;
    }
    lastSrcRef.current = src;
    // An app switch reuses this frame, so the incoming app reopens the same
    // white-canvas gap the initial mount had: cover it again until the new
    // document's `load`. Only after a navigation actually started, so a frame
    // that failed to navigate keeps showing the app it still has.
    setLoaded(false);
  }, [src]);

  return (
    <>
      <iframe
        ref={iframeRef}
        data-role="app-ui-frame"
        class="app-ui-iframe"
        src={initialSrc}
        sandbox="allow-scripts allow-same-origin allow-popups allow-forms allow-modals allow-popups-to-escape-sandbox"
        allow="autoplay; fullscreen; encrypted-media"
        onLoad={() => setLoaded(true)}
      />
      {coverMounted && (
        <div class={`app-ui-cover${loaded ? ' is-clearing' : ''}`} aria-hidden="true" />
      )}
    </>
  );
}

export function AppUiInline({ layout }: { layout: 'desktop' | 'mobile' }) {
  const app = currentApp.value;
  const refreshKey = appRefreshKey.value;
  const isPseudo = appPseudoFullscreen.value;
  // Skip mounting the iframe in the inactive dual-rendered layout — otherwise
  // every app open spawns two iframes loading the same id.
  const isActiveLayout = layout === (viewportIsMobile.value ? 'mobile' : 'desktop');

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

  // `key={refreshKey}` forces a fresh AppFrame on each refresh — keeping it
  // resets useState so the new iframe mounts with the cache-busted URL as its
  // initial src (no double-load). App switches keep the same key, so the
  // iframe element is reused and the URL change goes through location.replace.
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
      {frameSrc && <AppFrame key={refreshKey} src={frameSrc} />}
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
