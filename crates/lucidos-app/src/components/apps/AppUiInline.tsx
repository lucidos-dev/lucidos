import { useRef, useLayoutEffect } from 'preact/hooks';
import { currentApp, appPseudoFullscreen, appRefreshKey } from '../../store/store';
import { getAppFrameSrc, exitPseudoFullscreen } from '../../store/actions/apps';
import { ExitFullscreenIcon } from '../shared/icons';
import { viewportIsMobile } from '../../utils/viewport';

/** Append a cache-busting query param to a URL. */
function cacheBust(url: string, key: number): string {
  const u = new URL(url, window.location.origin);
  u.searchParams.set('_r', String(key));
  return u.toString();
}

export function AppUiInline({ layout }: { layout: 'desktop' | 'mobile' }) {
  const app = currentApp.value;
  const iframeRef = useRef<HTMLIFrameElement>(null);
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
  // `key={refreshKey}` forces a fresh iframe element on each refresh — flipping
  // `src` alone leaves listeners, observers, and SSE connections hanging.
  return (
    <div class={`app-ui-inline${isPseudo ? ' app-ui-fullscreen' : ''}`}>
      {isPseudo && (
        <button class="pseudo-fullscreen-exit icon-btn" onClick={exitPseudoFullscreen} aria-label="Exit fullscreen">
          <ExitFullscreenIcon />
        </button>
      )}
      <iframe
        key={refreshKey}
        ref={iframeRef}
        data-role="app-ui-frame"
        class={`app-ui-iframe${refreshKey > 0 ? ' app-iframe-refreshing' : ''}`}
        src={frameSrc || 'about:blank'}
        sandbox="allow-scripts allow-same-origin allow-popups allow-forms allow-modals allow-popups-to-escape-sandbox"
        allow="autoplay; fullscreen; encrypted-media"
        onLoad={() => {
          iframeRef.current?.classList.remove('app-iframe-refreshing');
        }}
      />
    </div>
  );
}
