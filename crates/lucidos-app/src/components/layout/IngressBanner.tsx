import type { Ref, VNode } from 'preact';
import { useRef } from 'preact/hooks';
import type { WebhookIngressOutage } from '../../api/client';
import { currentIngressOutage, ingressIsUnverified } from '../../store/actions/webhookIngress';
import { discussWebhookIngress } from '../../store/actions/webhook-ingress-discuss';
import { openWebhookSettings } from '../../store/actions/menu';
import { composeHandlers } from '../chat/composeHandlers';
import { useCoarseClock } from '../../hooks/useCoarseClock';
import { UNVERIFIED_INGRESS_NOTICE, webhookIngressNotice } from '../../utils/webhookIngressNotice';
import { viewportIsMobile } from '../../utils/viewport';
import { bannerBelongsToLayout, useBannerHeightVar, type BannerLayout } from './appBanner';
import { SurfaceToneIcon } from '../shared/Surface';

export const INGRESS_BANNER_HEIGHT_VAR = '--app-ingress-banner-height';

export function shouldRenderIngressBanner(opts: {
  layout: BannerLayout;
  mobileViewport: boolean;
  outage: WebhookIngressOutage | null;
  unverified?: boolean;
}): boolean {
  return bannerBelongsToLayout(opts.layout, opts.mobileViewport) && (opts.outage !== null || opts.unverified === true);
}

export function ingressBannerBody(props: {
  layout: BannerLayout;
  outage: WebhookIngressOutage | null;
  unverified?: boolean;
  onOpenWebhooks: () => void;
  onDiscuss: () => void;
  elRef?: Ref<HTMLDivElement>;
}): VNode | null {
  if (!props.outage && !props.unverified) return null;
  const notice = props.outage ? webhookIngressNotice(props.outage) : null;
  const discussHandlers = composeHandlers(props.onDiscuss, () => {});
  return (
    <div ref={props.elRef} class="ingress-banner" data-layout={props.layout}
      data-state={notice ? 'degraded' : 'unverified'} role="status">
      <SurfaceToneIcon tone={notice ? 'error' : 'info'} />
      <span class="ingress-banner-text">
        {notice && <b>{notice.title}</b>}
        {notice ? ` ${notice.detail}` : UNVERIFIED_INGRESS_NOTICE}
      </span>
      {notice && (
        <button class="action-btn action-btn-secondary" {...discussHandlers}>
          Discuss
        </button>
      )}
      <button class="action-btn action-btn-secondary" onClick={props.onOpenWebhooks}>
        Open Webhooks
      </button>
    </div>
  );
}

export function IngressBanner({ layout }: { layout: BannerLayout }) {
  const ref = useRef<HTMLDivElement>(null);
  const outage = currentIngressOutage(useCoarseClock());
  const unverified = ingressIsUnverified();
  const show = shouldRenderIngressBanner({
    layout,
    mobileViewport: viewportIsMobile.value,
    outage,
    unverified,
  });

  useBannerHeightVar(ref, { layout, cssVar: INGRESS_BANNER_HEIGHT_VAR, active: show });

  if (!show) return null;

  return ingressBannerBody({
    layout,
    outage,
    unverified,
    elRef: ref,
    onOpenWebhooks: openWebhookSettings,
    onDiscuss: () => { void discussWebhookIngress(outage!); },
  });
}
