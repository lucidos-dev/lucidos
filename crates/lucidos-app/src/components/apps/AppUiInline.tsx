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

const REFRESH_SETTLE_SLACK_MS = 500;

let framesMounted = 0;

let frameLoadWaiters: Array<{ after: number; settle: () => void }> = [];

function settleFrameLoad(frame: number): void {
  const waiting = frameLoadWaiters;
  frameLoadWaiters = waiting.filter((w) => w.after >= frame);
  for (const w of waiting) if (w.after < frame) w.settle();
}

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

function ContentAppFrame({ src, reveal, appId }: { src: string; reveal: AppReveal; appId: string }) {
  const [frame] = useState(() => ++framesMounted);
  return (
    <AppFrame
      src={src}
      reveal={reveal}
      label={{ app: appId, place: 'canvas' }}
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
  const isActiveLayout = layout === (viewportIsMobile.value ? 'mobile' : 'desktop');
  usePanelRefresh(`app "${app?.name ?? ''}"`, app && isActiveLayout ? () => refreshOpenApp(reveal) : null);

  useLayoutEffect(() => {
    if (!isActiveLayout) return;
    document.documentElement.toggleAttribute('data-pseudo-fullscreen', isPseudo);
    return () => document.documentElement.removeAttribute('data-pseudo-fullscreen');
  }, [isPseudo, isActiveLayout]);

  useLayoutEffect(() => {
    if (isActiveLayout) syncAppFullscreenHost();
  });

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
          {layout === 'mobile' && <EdgeSwipeZones />}
        </>
      )}
      <FindBar surface="content" scope={`app:${app.id}`} placeholder="Find in app" />
      {frameSrc && (
        <ContentAppFrame key={`${refreshKey}:${splitFrameSrc(frameSrc).doc}`} src={frameSrc} reveal={reveal} appId={app.id} />
      )}
      <div class="app-overlay-layer" data-overlay-layer="" />
    </div>
  );
}
