import { useLayoutEffect, useRef, useState } from 'preact/hooks';
import { scaledDurationMs } from '../../utils/motion';

export const NAV_COVER_MOTIONS = {
  arrive: { class: 'nav-cover', animMs: 200 },
  dip: { class: 'nav-cover nav-cover-dip', animMs: 300 },
} as const;
export type NavCoverMotion = keyof typeof NAV_COVER_MOTIONS;
const NAV_COVER_SLACK_MS = 50;

export function navCoverFuseMs(motion: NavCoverMotion): number {
  return scaledDurationMs(NAV_COVER_MOTIONS[motion].animMs) + NAV_COVER_SLACK_MS;
}

function useArrivingView(viewKey: string | null, motion: NavCoverMotion): string | null {
  const [arriving, setArriving] = useState<string | null>(null);
  const seenKeyRef = useRef(viewKey);
  useLayoutEffect(() => {
    if (seenKeyRef.current === viewKey) return;
    seenKeyRef.current = viewKey;
    if (viewKey === null) { setArriving(null); return; }
    setArriving(viewKey);
    const fuse = setTimeout(() => setArriving(null), navCoverFuseMs(motion));
    return () => clearTimeout(fuse);
  }, [viewKey]);
  return arriving;
}

export function NavigationCover({ viewKey, motion = 'arrive' }: { viewKey: string | null; motion?: NavCoverMotion }) {
  const arriving = useArrivingView(viewKey, motion);
  return arriving === null ? null : <div key={arriving} class={NAV_COVER_MOTIONS[motion].class} aria-hidden="true" />;
}
