import { useRef, useLayoutEffect } from 'preact/hooks';
import { currentApp, appPseudoFullscreen, appRefreshKey } from '../../store/store';
import { getAppFrameSrc, exitPseudoFullscreen } from '../../store/actions/apps';
import { ExitFullscreenIcon } from '../shared/icons';

/** Append a cache-busting query param to a URL. */
function cacheBust(url: string, key: number): string {
  const u = new URL(url, window.location.origin);
  u.searchParams.set('_r', String(key));
  return u.toString();
}

export function AppUiInline() {
  const app = currentApp.value;
  const iframeRef = useRef<HTMLIFrameElement>(null);
  const refreshKey = appRefreshKey.value;
  const isPseudo = appPseudoFullscreen.value;

  useLayoutEffect(() => {
    document.documentElement.toggleAttribute('data-pseudo-fullscreen', isPseudo);
    return () => document.documentElement.removeAttribute('data-pseudo-fullscreen');
  }, [isPseudo]);

  if (!app) return null;

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
