import type { Ref, VNode } from 'preact';
import { useRef } from 'preact/hooks';
import { connectionStatus, visibleWorkspaceName } from '../../store/store';
import type { ConnectionStatus } from '../../store/types';
import { connectionNotice } from '../../utils/connectionNotice';
import { viewportIsMobile } from '../../utils/viewport';
import { useDelayedFlag } from '../../hooks/useDelayedLoading';
import { bannerBelongsToLayout, useBannerHeightVar, type BannerLayout } from './appBanner';

/** The CSS custom property this banner publishes its measured height into. Its
 *  own, not the backup reminder's: both bars can be up at once, and
 *  `--app-header-bottom` sums them (see `useBannerHeightVar`). */
export const CONNECTION_BANNER_HEIGHT_VAR = '--app-conn-banner-height';

/** How long `connecting` must hold before the bar says so.
 *
 *  The two degraded states arrive with very different weight, and only one of
 *  them needs a fuse. `disconnected` is SETTLED by the time it exists:
 *  `runConnectionCheck` needs `MAX_SUPPRESSED_FAILURES + 1` consecutive failed
 *  polls to reach it, roughly 20s, precisely so a radio nap never paints red. So
 *  the bar shows it at once and adds no second delay to a wait the user has
 *  already served.
 *
 *  `connecting` is the opposite: it is the state before the FIRST `/health`
 *  answer and normally resolves in one tick, so announcing it immediately would
 *  put a bar on screen during an ordinary load. Past this fuse it is no longer
 *  an ordinary load. Longer than the 5s poll interval, so at least one poll has
 *  been tried and failed to land before the bar claims anything. */
export const CONNECTING_QUIET_MS = 8000;

/** Whether THIS instance renders: the mounted layout's, in a state the notice
 *  table speaks for, past the fuse if the state is the transient one.
 *
 *  Presence is keyed on the TABLE rather than on a list of state words, so the
 *  bar appears for exactly the states the mark recedes in and a fourth state
 *  could not be added to one without the other. Pure, so every combination is
 *  unit-testable without a DOM. */
export function shouldRenderConnectionBanner(opts: {
  layout: BannerLayout;
  mobileViewport: boolean;
  status: ConnectionStatus;
  connectingSettled: boolean;
}): boolean {
  if (!bannerBelongsToLayout(opts.layout, opts.mobileViewport)) return false;
  // The workspace name is irrelevant to whether there is anything to say, and
  // passing null here keeps that explicit: a nameless workspace still gets a bar.
  if (connectionNotice(opts.status, null) === null) return false;
  return opts.status === 'connecting' ? opts.connectingSettled : true;
}

/** Pure markup for the bar, hook-free so the tests can invoke it directly
 *  (the `backupReminderBody` idiom). `elRef` lands on the bar ITSELF, so the
 *  flex child the shell (and the mobile header) lays out is the same box the
 *  ResizeObserver measures.
 *
 *  A STATEMENT, not a control. There is nothing to press: the sentence is the
 *  whole message, the menu notice it shares its words with is a statement for
 *  the same reason, and neither remedy in the menu can fix a disconnect anyway
 *  (see `connectionNotice`). It is not dismissable either, which is the
 *  difference between this and the backup reminder: that one stays true until
 *  the user acts, so it needs a way out, while this one retracts itself on the
 *  next good poll and a dismissal would only hide a live fault.
 *
 *  `role="status"` rather than `alert`: the state is already announced by the
 *  mark's accessible name, and this is polite news about a condition, not an
 *  interruption. The dot is `aria-hidden` because the words beside it say the
 *  same thing. */
export function connectionBannerBody(props: {
  layout: BannerLayout;
  status: ConnectionStatus;
  workspace: string | null;
  elRef?: Ref<HTMLDivElement>;
}): VNode | null {
  const notice = connectionNotice(props.status, props.workspace);
  if (!notice) return null;
  return (
    <div
      ref={props.elRef}
      class="connection-banner"
      data-layout={props.layout}
      data-conn={props.status}
      role="status"
    >
      <span class={`status-dot ${props.status}`} aria-hidden="true" />
      <span class="connection-banner-text">
        <b>{notice.title}</b>{' '}{notice.detail}
      </span>
    </div>
  );
}

/**
 * The words for a connection that has gone bad, on screen rather than behind a
 * tap.
 *
 * The Lucidos mark is the connection light, and it says its state in strength
 * and motion alone (styles/header-mark.css). That is a good signal and a poor
 * message: it cannot say WHAT is wrong, its desktop tooltip is hover-only, and
 * the menu notice that does explain it is behind opening the menu. This is the
 * half that needs no gesture at all.
 *
 * It is a bar under the header row rather than text beside the mark because the
 * mark sits in `.header-nav-cluster`, a fixed-width box centred on the row's
 * axis whose clearance from both edge clusters is a structural guarantee pinned
 * by `e2e/mobile-threads-title-alignment.spec.ts`. Text added there would push
 * the mark off that axis and spend clearance the spec measures, and it would
 * have to be short enough to say nothing. A bar costs the row's width budget
 * nothing at any viewport.
 *
 * Two mount points, one per layout, per the dual-render rule: a flow sibling in
 * `.app-shell` on desktop, and a child of the fixed `.app-header` on mobile,
 * where a flow sibling would sit behind the header.
 */
export function ConnectionBanner({ layout }: { layout: BannerLayout }) {
  const ref = useRef<HTMLDivElement>(null);
  const status = connectionStatus.value;
  const connectingSettled = useDelayedFlag(status === 'connecting', CONNECTING_QUIET_MS);
  const show = shouldRenderConnectionBanner({
    layout,
    mobileViewport: viewportIsMobile.value,
    status,
    connectingSettled,
  });

  useBannerHeightVar(ref, { layout, cssVar: CONNECTION_BANNER_HEIGHT_VAR, active: show });

  if (!show) return null;

  return connectionBannerBody({
    layout,
    status,
    workspace: visibleWorkspaceName.value,
    elRef: ref,
  });
}
