import type { Ref, VNode } from 'preact';
import { useRef } from 'preact/hooks';
import { viewportIsMobile } from '../../utils/viewport';
import { bannerBelongsToLayout, useBannerHeightVar, type BannerLayout } from './appBanner';
import { SurfaceToneIcon } from '../shared/Surface';

export const PEER_CLIENT_BANNER_HEIGHT_VAR = '--app-peer-client-banner-height';
export const PEER_CLIENT_UNAVAILABLE_META = 'lucidos-peer-client-unavailable';

export function unavailablePeerClient(doc: Pick<Document, 'querySelector'> | undefined): string | null {
  return doc?.querySelector(`meta[name="${PEER_CLIENT_UNAVAILABLE_META}"]`)?.getAttribute('content') || null;
}

const UNAVAILABLE_PEER: string | null = unavailablePeerClient(
  typeof document === 'undefined' ? undefined : document,
);

export function peerClientBannerBody(props: {
  layout: BannerLayout;
  peer: string | null;
  elRef?: Ref<HTMLDivElement>;
}): VNode | null {
  if (props.peer === null) return null;
  return (
    <div ref={props.elRef} class="peer-client-banner" data-layout={props.layout} role="status">
      <SurfaceToneIcon tone="warning" />
      <span class="peer-client-banner-text">
        <b>Showing this gateway's own client.</b>{' '}
        Trusted pairing is on for "{props.peer}", but it could not serve its own client.
      </span>
    </div>
  );
}

export function PeerClientBanner({ layout }: { layout: BannerLayout }) {
  const ref = useRef<HTMLDivElement>(null);
  const show = bannerBelongsToLayout(layout, viewportIsMobile.value) && UNAVAILABLE_PEER !== null;

  useBannerHeightVar(ref, { layout, cssVar: PEER_CLIENT_BANNER_HEIGHT_VAR, active: show });

  if (!show) return null;
  return peerClientBannerBody({ layout, peer: UNAVAILABLE_PEER, elRef: ref });
}
