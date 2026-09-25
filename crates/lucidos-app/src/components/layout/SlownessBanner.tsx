import type { ComponentChildren, Ref, VNode } from 'preact';
import { useRef } from 'preact/hooks';
import {
  dismissedSlownessEpisode,
  dismissSlownessEpisode,
  slownessStatus,
  visibleSlownessEpisode,
  type SlownessEpisode,
} from '../../store/actions/slowness';
import { WORKSPACE_ID } from '../../utils/basePath';
import {
  busyEnoughToName,
  memoryRecommendation,
  memoryUsersPhrase,
  processorRecommendation,
  processorUsersPhrase,
} from '../../utils/slownessNotice';
import { viewportIsMobile } from '../../utils/viewport';
import { CloseIcon } from '../shared/icons';
import { bannerBelongsToLayout, useBannerHeightVar, type BannerLayout } from './appBanner';

/** The CSS custom property this banner publishes its measured height into. Its
 *  own, since every bar can be up at once (see `useBannerHeightVar`). */
export const SLOWNESS_BANNER_HEIGHT_VAR = '--app-slowness-banner-height';

/** Whether THIS instance renders: the mounted layout's, with an episode to
 *  show. Pure, so both halves are unit-testable without a DOM. */
export function shouldRenderSlownessBanner(opts: {
  layout: BannerLayout;
  mobileViewport: boolean;
  episode: SlownessEpisode | null;
}): boolean {
  return bannerBelongsToLayout(opts.layout, opts.mobileViewport) && opts.episode !== null;
}

/** The bar's sentence: what is wrong, who to look at, and one thing to do.
 *
 *  "The computer running Lucidos", never "this Mac": a phone reaching the
 *  workspace remotely reads the same bar about a machine it is not. The
 *  unclear reason names no cause at all, because none was measured. */
function slownessSentence(episode: SlownessEpisode): ComponentChildren {
  if (episode.reason === 'memory') {
    const users = episode.top_users;
    return [
      <b>The computer running Lucidos is short on memory, so Lucidos is slow.</b>,
      users.length > 0 ? ` Biggest users: ${memoryUsersPhrase(users)}.` : '',
      ` ${memoryRecommendation(users)}`,
    ];
  }
  const busiest = busyEnoughToName(episode.busiest_apps);
  return [
    <b>Lucidos is responding slowly.</b>,
    busiest.length > 0 ? ` Busiest apps: ${processorUsersPhrase(busiest)}.` : '',
    ` ${processorRecommendation(episode.busiest_apps)}`,
  ];
}

/** Pure markup for the bar. `elRef` lands on the bar itself, so the box the
 *  shell lays out is the box the ResizeObserver measures. */
export function slownessBannerBody(props: {
  layout: BannerLayout;
  episode: SlownessEpisode;
  onDismiss: () => void;
  elRef?: Ref<HTMLDivElement>;
}): VNode {
  return (
    <div ref={props.elRef} class="slowness-banner" data-layout={props.layout} role="status">
      <span class="slowness-banner-text">{slownessSentence(props.episode)}</span>
      <button
        class="icon-btn slowness-banner-close"
        onClick={props.onDismiss}
        aria-label="Dismiss slowness warning"
      >
        <CloseIcon />
      </button>
    </div>
  );
}

/**
 * The slowness warning (ADRs 0274, 0283). The gateway opens an episode only
 * when slowness or memory pressure has held for five minutes, so the bar is
 * news rather than noise.
 *
 * A bar rather than a toast: the condition stays true until something frees
 * the machine, and it explains every slow turn while it lasts. Dismissing hides
 * this episode on this device; the next episode shows again.
 *
 * Two mount points, one per layout, per the dual-render rule.
 */
export function SlownessBanner({ layout }: { layout: BannerLayout }) {
  const ref = useRef<HTMLDivElement>(null);
  const episode = visibleSlownessEpisode(
    slownessStatus.value,
    dismissedSlownessEpisode.value,
    WORKSPACE_ID,
  );
  const show = shouldRenderSlownessBanner({
    layout,
    mobileViewport: viewportIsMobile.value,
    episode,
  });

  useBannerHeightVar(ref, { layout, cssVar: SLOWNESS_BANNER_HEIGHT_VAR, active: show });

  if (!show || !episode) return null;

  return slownessBannerBody({
    layout,
    episode,
    elRef: ref,
    onDismiss: () => dismissSlownessEpisode(episode.episode_id),
  });
}
