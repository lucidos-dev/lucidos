import type { Ref, VNode } from 'preact';
import { useRef } from 'preact/hooks';
import type { WebhookRefusal } from '../../api/client';
import {
  currentWebhookRefusals,
  loudestWebhookRefusal,
} from '../../store/actions/webhookRefusals';
import { discussWebhookRefusal } from '../../store/actions/webhook-refusal-discuss';
import { openWebhookSettings } from '../../store/actions/menu';
import { composeHandlers } from '../chat/composeHandlers';
import { useCoarseClock } from '../../hooks/useCoarseClock';
import { webhookRefusalNotice } from '../../utils/webhookRefusalNotice';
import { viewportIsMobile } from '../../utils/viewport';
import { bannerBelongsToLayout, useBannerHeightVar, type BannerLayout } from './appBanner';
import { SurfaceToneIcon } from '../shared/Surface';

export const REFUSAL_BANNER_HEIGHT_VAR = '--app-refusal-banner-height';

export function shouldRenderRefusalBanner(opts: {
  layout: BannerLayout;
  mobileViewport: boolean;
  refusal: WebhookRefusal | null;
}): boolean {
  return bannerBelongsToLayout(opts.layout, opts.mobileViewport) && opts.refusal !== null;
}

export function otherRefusalsPhrase(refusals: WebhookRefusal[]): string | null {
  const others = refusals.length - 1;
  if (others < 1) return null;
  return others === 1 ? '1 other webhook is too.' : `${others} other webhooks are too.`;
}

export function refusalBannerBody(props: {
  layout: BannerLayout;
  refusal: WebhookRefusal | null;
  others: string | null;
  onOpenWebhooks: () => void;
  onDiscuss: () => void;
  elRef?: Ref<HTMLDivElement>;
}): VNode | null {
  if (!props.refusal) return null;
  const notice = webhookRefusalNotice(props.refusal);
  const discussHandlers = composeHandlers(props.onDiscuss, () => {});
  return (
    <div
      ref={props.elRef}
      class="refusal-banner"
      data-layout={props.layout}
      data-cause={props.refusal.cause}
      role="status"
    >
      <SurfaceToneIcon tone="error" />
      <span class="refusal-banner-text">
        <b>{notice.title}</b>{' '}{notice.detail}
        {props.others ? ` ${props.others}` : ''}
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

export function WebhookRefusalBanner({ layout }: { layout: BannerLayout }) {
  const ref = useRef<HTMLDivElement>(null);
  const refusals = currentWebhookRefusals(useCoarseClock());
  const refusal = loudestWebhookRefusal(refusals);
  const others = otherRefusalsPhrase(refusals);
  const show = shouldRenderRefusalBanner({
    layout,
    mobileViewport: viewportIsMobile.value,
    refusal,
  });

  useBannerHeightVar(ref, { layout, cssVar: REFUSAL_BANNER_HEIGHT_VAR, active: show });

  if (!show) return null;

  return refusalBannerBody({
    layout,
    refusal,
    others,
    elRef: ref,
    onOpenWebhooks: openWebhookSettings,
    onDiscuss: () => { void discussWebhookRefusal(refusal!, others); },
  });
}
