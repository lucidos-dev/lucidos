import { useRef, useLayoutEffect, useState, useEffect, useMemo } from 'preact/hooks';
import { scaledDurationMs } from '../../store/store';
import { useDelayedFlag, useLingeringFlag } from '../../hooks/useDelayedLoading';
import { setAppFrameHash, splitFrameSrc } from './iframeNav';
import { APP_FRAME_SANDBOX, APP_FRAME_ALLOW } from './appFrameSandbox';
import { pushKeybindingsToFrame } from '../../store/actions/app-keybindings';
import { pushExternalLinksToFrame } from '../../store/actions/app-external-links';
import { pushAppearanceToFrame } from '../../store/actions/app-appearance';
import { IframeTabExit } from '../shared/IframeTabExit';
import { pushScaleWheelArmedToFrame } from '../shared/scaleWheelListener';
import { APP_FRAME_READY_EVENT } from '../../store/actions/app-ready-bridge';
import type { AppReveal } from '../../store/types';
import { appFrameLoading, appFrameRevealed, revealFuseMs } from './appFrameReveal';
import { frameLoaded, frameUnmounted, trackFrameLoad } from './appFrameSharedLoads';
import { announceFrameGone } from '../../store/actions/frame-swipe-bridge';
import { startFrameLoadProbe, type FrameLabel, type FrameLoadProbe } from './appFrameLoadProbe';

const COVER_FADE_MS = 200;
const COVER_FADE_SLACK_MS = 50;

export function cacheBust(url: string, key: number): string {
  const u = new URL(url, window.location.origin);
  u.searchParams.set('_r', String(key));
  return u.toString();
}

export interface AppFrameProps {
  src: string;
  reveal: AppReveal;
  label: FrameLabel;
  onRevealed?: () => void;
  onLoad?: (frame: HTMLIFrameElement) => void;
  onUnmount?: () => void;
}

export function AppFrame({ src, reveal, label, onRevealed, onLoad, onUnmount }: AppFrameProps) {
  const iframeRef = useRef<HTMLIFrameElement>(null);
  const probeRef = useRef<FrameLoadProbe | null>(null);
  const [frameSrc, setFrameSrc] = useState(src);
  const lastSrcRef = useRef(frameSrc);
  const [generation, setGeneration] = useState(0);

  const [loaded, setLoaded] = useState(false);
  const [ready, setReady] = useState(false);
  const [fused, setFused] = useState(false);
  const [reload] = useState(() => () => {
    setFrameSrc(lastSrcRef.current);
    setReady(false);
    setFused(false);
    setGeneration((g) => g + 1);
  });
  const signals = { loaded, ready, fused };
  const revealed = appFrameRevealed(reveal, signals);
  const coverMounted = useLingeringFlag(!revealed, scaledDurationMs(COVER_FADE_MS) + COVER_FADE_SLACK_MS);
  const barShown = useDelayedFlag(appFrameLoading(reveal, signals));
  const barMounted = useLingeringFlag(barShown, scaledDurationMs(COVER_FADE_MS) + COVER_FADE_SLACK_MS);

  useEffect(() => () => onUnmount?.(), []);

  useLayoutEffect(() => {
    trackFrameLoad(reload);
    return () => frameUnmounted(reload);
  }, []);

  const attachedAt = useMemo(() => performance.now(), [generation]);

  useLayoutEffect(() => {
    const probe = startFrameLoadProbe(label, () => iframeRef.current, attachedAt);
    probeRef.current = probe;
    return () => {
      probe.abandon();
      probeRef.current = null;
    };
  }, [generation]);

  useLayoutEffect(() => {
    const iframe = iframeRef.current;
    if (!iframe) return;
    const onReady = () => {
      probeRef.current?.noteReady();
      setReady(true);
    };
    iframe.addEventListener(APP_FRAME_READY_EVENT, onReady);
    return () => iframe.removeEventListener(APP_FRAME_READY_EVENT, onReady);
  }, [generation]);

  useLayoutEffect(() => {
    const frameWindow = iframeRef.current?.contentWindow ?? null;
    return () => announceFrameGone(frameWindow);
  }, [generation]);

  useEffect(() => {
    if (revealed) {
      probeRef.current?.noteRevealed(fused);
      onRevealed?.();
      return;
    }
    const fuse = setTimeout(() => setFused(true), revealFuseMs(reveal));
    return () => clearTimeout(fuse);
  }, [revealed, generation]);

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
        key={generation}
        ref={iframeRef}
        data-role="app-ui-frame"
        class="app-ui-iframe"
        src={frameSrc}
        sandbox={APP_FRAME_SANDBOX}
        allow={APP_FRAME_ALLOW}
        onLoad={(e) => {
          setLoaded(true);
          probeRef.current?.noteLoad();
          frameLoaded(reload);
          pushKeybindingsToFrame(e.currentTarget as HTMLIFrameElement);
          pushExternalLinksToFrame(e.currentTarget as HTMLIFrameElement);
          pushAppearanceToFrame(e.currentTarget as HTMLIFrameElement);
          pushScaleWheelArmedToFrame(e.currentTarget as HTMLIFrameElement);
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
