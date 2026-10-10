import { useRef, useLayoutEffect, useState, useEffect } from 'preact/hooks';
import { scaledDurationMs } from '../../store/store';
import { useDelayedFlag, useLingeringFlag } from '../../hooks/useDelayedLoading';
import { setAppFrameHash, splitFrameSrc } from './iframeNav';
import { APP_FRAME_SANDBOX, APP_FRAME_ALLOW } from './appFrameSandbox';
import { pushKeybindingsToFrame } from '../../store/actions/app-keybindings';
import { pushAppearanceToFrame } from '../../store/actions/app-appearance';
import { IframeTabExit } from '../shared/IframeTabExit';
import { APP_FRAME_READY_EVENT } from '../../store/actions/app-ready-bridge';
import type { AppReveal } from '../../store/types';
import { appFrameLoading, appFrameRevealed, revealFuseMs } from './appFrameReveal';

/** The load cover's CSS opacity transition at 1x (var(--duration-normal)).
 *  The cover lingers for this, scaled by the Animation speed slider, plus
 *  fixed slack. So it stays mounted until the fade finishes at any setting,
 *  as the LoadingFade component holds a clearing skeleton. */
const COVER_FADE_MS = 200;
const COVER_FADE_SLACK_MS = 50;

/** Append a cache-busting query param to a URL. */
export function cacheBust(url: string, key: number): string {
  const u = new URL(url, window.location.origin);
  u.searchParams.set('_r', String(key));
  return u.toString();
}

export interface AppFrameProps {
  src: string;
  reveal: AppReveal;
  /** Runs once the cover lifts. */
  onRevealed?: () => void;
  /** Runs on every load, after the host's own pushes. */
  onLoad?: (frame: HTMLIFrameElement) => void;
  /** Runs when this element goes away. */
  onUnmount?: () => void;
}

/** The app iframe element: the one place an app frame's rules live. The
 *  Canvas pane and a widget on the shelf both draw this, so a widget's frame is an
 *  app's frame, attribute for attribute (ADR 0402).
 *
 *  Freezes the JSX `src` at mount so Preact never diffs it. A live element must
 *  not have its `src` mutated: that adds an entry to iOS Safari's joint session
 *  history (WebKit #9166). The PWA's edge-swipe-back gesture then surfaces a
 *  snapshot of a previous app state mid-swipe. An app switch remounts this
 *  component instead, and a first load adds no entry. */
export function AppFrame({ src, reveal, onRevealed, onLoad, onUnmount }: AppFrameProps) {
  const iframeRef = useRef<HTMLIFrameElement>(null);
  const [initialSrc] = useState(src);
  const lastSrcRef = useRef(initialSrc);

  // A frame with no document yet paints its base canvas, which WKWebView fills
  // WHITE. On a dark theme every app open flashed white first. The app's own
  // stylesheet had not landed, nor `/api/v1/sdk-prefs.js`, the second request
  // that actually applies the theme. Both gaps are inside a document the host
  // does not author, so the host hides the frame instead: an opaque
  // theme-coloured cover from mount, crossfaded out once the frame has
  // something to show. AppUiInline.test.ts says why the cover is a sibling.
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

  useEffect(() => () => onUnmount?.(), []);

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
      onRevealed?.();
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
          onLoad?.(e.currentTarget as HTMLIFrameElement);
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
