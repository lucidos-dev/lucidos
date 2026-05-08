import { useRef, useLayoutEffect, useState } from 'preact/hooks';
import { currentApp, appPseudoFullscreen, appRefreshKey } from '../../store/store';
import { getAppFrameSrc, exitPseudoFullscreen } from '../../store/actions/apps';
import { ExitFullscreenIcon } from '../shared/icons';
import { viewportIsMobile } from '../../utils/viewport';
import { navigateAppIframe } from './iframeNav';

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
function AppFrame({ src, refreshing }: { src: string; refreshing: boolean }) {
  const iframeRef = useRef<HTMLIFrameElement>(null);
  const [initialSrc] = useState(src);
  const lastSrcRef = useRef(initialSrc);

  useLayoutEffect(() => {
    const iframe = iframeRef.current;
    if (!iframe) return;
    if (lastSrcRef.current === src) return;
    navigateAppIframe(iframe, src);
    lastSrcRef.current = src;
  }, [src]);

  return (
    <iframe
      ref={iframeRef}
      data-role="app-ui-frame"
      class={`app-ui-iframe${refreshing ? ' app-iframe-refreshing' : ''}`}
      src={initialSrc}
      sandbox="allow-scripts allow-same-origin allow-popups allow-forms allow-modals allow-popups-to-escape-sandbox"
      allow="autoplay; fullscreen; encrypted-media"
      onLoad={() => {
        iframeRef.current?.classList.remove('app-iframe-refreshing');
      }}
    />
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

  if (!app) return null;
  if (!isActiveLayout) return null;

  const baseSrc = getAppFrameSrc();
  const frameSrc = (baseSrc && refreshKey > 0) ? cacheBust(baseSrc, refreshKey) : baseSrc;

  // `key={refreshKey}` forces a fresh AppFrame on each refresh — keeping it
  // resets useState so the new iframe mounts with the cache-busted URL as its
  // initial src (no double-load). App switches keep the same key, so the
  // iframe element is reused and the URL change goes through location.replace.
  return (
    <div class={`app-ui-inline${isPseudo ? ' app-ui-fullscreen' : ''}`}>
      {isPseudo && (
        <button class="pseudo-fullscreen-exit icon-btn" onClick={exitPseudoFullscreen} aria-label="Exit fullscreen">
          <ExitFullscreenIcon />
        </button>
      )}
      {frameSrc && <AppFrame key={refreshKey} src={frameSrc} refreshing={refreshKey > 0} />}
    </div>
  );
}
