import type { Ref, VNode } from 'preact';
import { useRef } from 'preact/hooks';
import type { WebhookIngressOutage } from '../../api/client';
import { currentIngressOutage } from '../../store/actions/webhookIngress';
import { discussWebhookIngress } from '../../store/actions/webhook-ingress-discuss';
import { openWebhookSettings } from '../../store/actions/menu';
import { composeHandlers } from '../chat/composeHandlers';
import { useCoarseClock } from '../../hooks/useCoarseClock';
import { webhookIngressNotice } from '../../utils/webhookIngressNotice';
import { viewportIsMobile } from '../../utils/viewport';
import { bannerBelongsToLayout, useBannerHeightVar, type BannerLayout } from './appBanner';
import { SurfaceToneIcon } from '../shared/Surface';

export const INGRESS_BANNER_HEIGHT_VAR = '--app-ingress-banner-height';

export function shouldRenderIngressBanner(opts: {
  layout: BannerLayout;
  mobileViewport: boolean;
  outage: WebhookIngressOutage | null;
}): boolean {
  return bannerBelongsToLayout(opts.layout, opts.mobileViewport) && opts.outage !== null;
}

export function ingressBannerBody(props: {
  layout: BannerLayout;
  outage: WebhookIngressOutage | null;
  onOpenWebhooks: () => void;
  onDiscuss: () => void;
  elRef?: Ref<HTMLDivElement>;
}): VNode | null {
  if (!props.outage) return null;
  const notice = webhookIngressNotice(props.outage);
  const discussHandlers = composeHandlers(props.onDiscuss, () => {});
  return (
    <div ref={props.elRef} class="ingress-banner" data-layout={props.layout} role="status">
      <SurfaceToneIcon tone="error" />
      <span class="ingress-banner-text">
        <b>{notice.title}</b>{' '}{notice.detail}
      </span>
      <button class="action-btn action-btn-secondary" {...discussHandlers}>
        Discuss
      </button>
      <button class="action-btn action-btn-secondary" onClick={props.onOpenWebhooks}>
        Open Webhooks
      </button>
    </div>
  );
}

export function IngressBanner({ layout }: { layout: BannerLayout }) {
  const ref = useRef<HTMLDivElement>(null);
  const outage = currentIngressOutage(useCoarseClock());
  const show = shouldRenderIngressBanner({
    layout,
    mobileViewport: viewportIsMobile.value,
    outage,
  });

  useBannerHeightVar(ref, { layout, cssVar: INGRESS_BANNER_HEIGHT_VAR, active: show });

  if (!show) return null;

  return ingressBannerBody({
    layout,
    outage,
    elRef: ref,
    onOpenWebhooks: openWebhookSettings,
    onDiscuss: () => { void discussWebhookIngress(outage!); },
  });
}
