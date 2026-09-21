import type { Ref, VNode } from 'preact';
import { useRef } from 'preact/hooks';
import type { WebhookRefusal } from '../../api/client';
import {
  currentWebhookRefusals,
  loudestWebhookRefusal,
} from '../../store/actions/webhookRefusals';
import { openWebhookSettings } from '../../store/actions/menu';
import { useCoarseClock } from '../../hooks/useCoarseClock';
import { webhookRefusalNotice } from '../../utils/webhookRefusalNotice';
import { viewportIsMobile } from '../../utils/viewport';
import { bannerBelongsToLayout, useBannerHeightVar, type BannerLayout } from './appBanner';

/** The CSS custom property this banner publishes its measured height into. Its
 *  own, not any neighbour's: every bar can be up at once, and
 *  `--app-header-bottom` sums them (see `useBannerHeightVar`). */
export const REFUSAL_BANNER_HEIGHT_VAR = '--app-refusal-banner-height';

/** Whether THIS instance renders: the mounted layout's, with a hook refusing.
 *
 *  No fuse. The engine spends a count of real deliveries and a half-hour clock
 *  before declaring. The news is settled by the time a client reads it.
 *  Pure, so both halves are unit-testable without a DOM. */
export function shouldRenderRefusalBanner(opts: {
  layout: BannerLayout;
  mobileViewport: boolean;
  refusal: WebhookRefusal | null;
}): boolean {
  return bannerBelongsToLayout(opts.layout, opts.mobileViewport) && opts.refusal !== null;
}

/** How the bar mentions the hooks it is not speaking for.
 *
 *  One bar, one sentence, however many hooks are broken. Naming them all would
 *  make the bar grow without telling the reader anything the page will not.
 *  Null for the ordinary case of exactly one. */
export function otherRefusalsPhrase(refusals: WebhookRefusal[]): string | null {
  const others = refusals.length - 1;
  if (others < 1) return null;
  return others === 1 ? '1 other webhook is too.' : `${others} other webhooks are too.`;
}

/** Pure markup for the bar, hook-free so the tests can invoke it directly (the
 *  `connectionBannerBody` idiom). `elRef` lands on the bar ITSELF, so the flex
 *  child the shell lays out is the same box the ResizeObserver measures.
 *
 *  One button, and it is the one that reaches the fix. A switched-off hook is
 *  re-enabled from the Webhooks page in a single click, and a signature fault
 *  is repaired from the same row. There is no Discuss here, unlike the ingress
 *  bar: that fault needs a diagnosis worked out address by address, and this
 *  one already names its own cause.
 *
 *  Not dismissable. The bar retracts itself the moment a delivery verifies, so
 *  a dismiss would only hide a live loss of data.
 *
 *  `role="status"` rather than `alert`: the condition has already held for at
 *  least half an hour, so it is news rather than an interruption. */
export function refusalBannerBody(props: {
  layout: BannerLayout;
  refusal: WebhookRefusal | null;
  others: string | null;
  onOpenWebhooks: () => void;
  elRef?: Ref<HTMLDivElement>;
}): VNode | null {
  if (!props.refusal) return null;
  const notice = webhookRefusalNotice(props.refusal);
  return (
    <div
      ref={props.elRef}
      class="refusal-banner"
      data-layout={props.layout}
      data-cause={props.refusal.cause}
      role="status"
    >
      <span class="refusal-banner-text">
        <b>{notice.title}</b>{' '}{notice.detail}
        {props.others ? ` ${props.others}` : ''}
      </span>
      <button class="action-btn" onClick={props.onOpenWebhooks}>
        Open Webhooks
      </button>
    </div>
  );
}

/**
 * The words for a webhook that is throwing its deliveries away.
 *
 * A bar rather than a notification, for the reason the ingress bar is one: the
 * fault is silent by nature. Nothing looked wrong for 18 days while every
 * delivery got a 401, so the news has to sit on screen without being asked for.
 *
 * It is a SEPARATE bar from the ingress one, not a second message inside it.
 * The two faults are independent and can stand together: the path can be down
 * while a hook is also switched off. They also want different actions, and a
 * bar that had to say both would say neither clearly.
 *
 * Two mount points, one per layout, per the dual-render rule: a flow sibling in
 * `.app-shell` on desktop, and a child of the fixed `.app-header` on mobile,
 * where a flow sibling would sit behind the header.
 *
 * See `docs/adr/0235-a-refused-delivery-is-an-outage.md`.
 */
export function WebhookRefusalBanner({ layout }: { layout: BannerLayout }) {
  const ref = useRef<HTMLDivElement>(null);
  // The shared clock, so the bar's age and every Webhooks row's agree exactly.
  const refusals = currentWebhookRefusals(useCoarseClock());
  const refusal = loudestWebhookRefusal(refusals);
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
    others: otherRefusalsPhrase(refusals),
    elRef: ref,
    onOpenWebhooks: openWebhookSettings,
  });
}
