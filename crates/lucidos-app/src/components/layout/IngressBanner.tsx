import type { Ref, VNode } from 'preact';
import { useRef } from 'preact/hooks';
import type { WebhookIngressOutage } from '../../api/client';
import { currentIngressOutage } from '../../store/actions/webhookIngress';
import { discussWebhookIngress } from '../../store/actions/webhook-ingress-discuss';
import { openWebhookSettings } from '../../store/actions/menu';
import { composeHandlers } from '../chat/promptFocus';
import { useCoarseClock } from '../../hooks/useCoarseClock';
import { webhookIngressNotice } from '../../utils/webhookIngressNotice';
import { viewportIsMobile } from '../../utils/viewport';
import { bannerBelongsToLayout, useBannerHeightVar, type BannerLayout } from './appBanner';

/** The CSS custom property this banner publishes its measured height into. Its
 *  own, not either neighbour's: all three bars can be up at once, and
 *  `--app-header-bottom` sums them (see `useBannerHeightVar`). */
export const INGRESS_BANNER_HEIGHT_VAR = '--app-ingress-banner-height';

/** Whether THIS instance renders: the mounted layout's, with an outage standing.
 *
 *  No fuse, unlike the connection bar's `connecting`. The engine spends two
 *  consecutive failed probe cycles before it declares anything, so the news is
 *  settled by the time a client can read it. Pure, so both halves are
 *  unit-testable without a DOM. */
export function shouldRenderIngressBanner(opts: {
  layout: BannerLayout;
  mobileViewport: boolean;
  outage: WebhookIngressOutage | null;
}): boolean {
  return bannerBelongsToLayout(opts.layout, opts.mobileViewport) && opts.outage !== null;
}

/** Pure markup for the bar, hook-free so the tests can invoke it directly (the
 *  `connectionBannerBody` idiom). `elRef` lands on the bar ITSELF, so the flex
 *  child the shell (and the mobile header) lays out is the same box the
 *  ResizeObserver measures.
 *
 *  No dot. The `.status-dot` scale names THIS client's connection, and borrowing
 *  a word from it here would say the app is offline while it is online.
 *
 *  Neither button repairs anything. The engine reports an ingress outage and
 *  never fixes one, so a button promising a fix would promise what nothing
 *  behind it does. One navigates to the page. The other hands the whole
 *  declaration to the agent, which is where a repair can be worked out. Not
 *  dismissable either: the bar retracts itself on the next good probe, so a
 *  dismiss would only hide a live fault.
 *
 *  `role="status"` rather than `alert`: this is news about a condition that has
 *  already held for two probe cycles, not an interruption. */
export function ingressBannerBody(props: {
  layout: BannerLayout;
  outage: WebhookIngressOutage | null;
  onOpenWebhooks: () => void;
  onDiscuss: () => void;
  elRef?: Ref<HTMLDivElement>;
}): VNode | null {
  if (!props.outage) return null;
  const notice = webhookIngressNotice(props.outage);
  // `composeHandlers` with NO focus nudge, the shape the notification detail's
  // Discuss uses. The wrapper is here for its touch and click dedup. A tap with
  // the iOS keyboard up can blur the field and shift the viewport, which moves
  // the button out from under the finger. WebKit then drops the synthetic
  // click. The focus half is dropped because Discuss sends, so a raised
  // keyboard would only cover the reply.
  const discussHandlers = composeHandlers(props.onDiscuss, () => {});
  return (
    <div ref={props.elRef} class="ingress-banner" data-layout={props.layout} role="status">
      <span class="ingress-banner-text">
        <b>{notice.title}</b>{' '}{notice.detail}
      </span>
      <button class="action-btn" {...discussHandlers}>
        Discuss
      </button>
      <button class="action-btn" onClick={props.onOpenWebhooks}>
        Open Webhooks
      </button>
    </div>
  );
}

/**
 * The words for a webhook ingress outage: deliveries cannot reach this workspace
 * from outside the machine, over at least one address family.
 *
 * A bar rather than a notification, because the fault is silent by nature. The
 * whole failure this catches is that nothing looked wrong: the page said the
 * hook was active, the trigger row stayed green, and events simply never
 * arrived. So the news has to sit on screen without being asked for.
 *
 * It reads its own state and never `connectionStatus`. That signal is this
 * client's health poll, and reusing it would claim the app is offline while it
 * is online. The two are close to opposites: an ingress outage is a workspace
 * everyone can reach except the senders that matter.
 *
 * Two mount points, one per layout, per the dual-render rule: a flow sibling in
 * `.app-shell` on desktop, and a child of the fixed `.app-header` on mobile,
 * where a flow sibling would sit behind the header.
 *
 * See `docs/adr/0143-webhook-ingress-probed-per-address-family.md`.
 */
export function IngressBanner({ layout }: { layout: BannerLayout }) {
  const ref = useRef<HTMLDivElement>(null);
  // The shared clock, so the bar's age and every Webhooks row's agree exactly.
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
    // Non-null inside `show`, and read here so the message quotes the outage
    // exactly as it is drawn, age included.
    onDiscuss: () => { void discussWebhookIngress(outage!); },
  });
}
