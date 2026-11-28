import type { ComponentChildren, Ref, VNode } from 'preact';
import { useRef } from 'preact/hooks';
import {
  dismissedSlownessEpisode,
  dismissSlownessEpisode,
  slownessStatus,
  visibleSlownessEpisode,
  type SlownessEpisode,
} from '../../store/actions/slowness';
import { enginePackaged } from '../../store/store';
import { WORKSPACE_ID } from '../../utils/basePath';
import {
  busyEnoughToName,
  databaseRecommendation,
  diskRecommendation,
  memoryRecommendation,
  memoryUsersPhrase,
  processorRecommendation,
  processorUsersPhrase,
} from '../../utils/slownessNotice';
import { viewportIsMobile } from '../../utils/viewport';
import { CloseIcon } from '../shared/icons';
import { SurfaceToneIcon } from '../shared/Surface';
import { bannerBelongsToLayout, useBannerHeightVar, type BannerLayout } from './appBanner';

export const SLOWNESS_BANNER_HEIGHT_VAR = '--app-slowness-banner-height';

export function shouldRenderSlownessBanner(opts: {
  layout: BannerLayout;
  mobileViewport: boolean;
  episode: SlownessEpisode | null;
}): boolean {
  return bannerBelongsToLayout(opts.layout, opts.mobileViewport) && opts.episode !== null;
}

function slownessSentence(episode: SlownessEpisode, packaged: boolean): ComponentChildren {
  switch (episode.reason) {
    case 'disk':
      return [
        <b>The computer running Lucidos is almost out of disk space, so Lucidos is slow.</b>,
        ` ${diskRecommendation(episode.free_bytes, packaged)}`,
      ];
    case 'database':
      return [
        <b>
          {episode.problem === 'pool_exhausted'
            ? 'Lucidos is waiting for a free database connection, so it is slow.'
            : "Lucidos's database is not responding, so Lucidos is slow."}
        </b>,
        ` ${databaseRecommendation(episode.problem, packaged)}`,
      ];
    case 'memory': {
      const users = episode.top_users;
      return [
        <b>The computer running Lucidos is short on memory, so Lucidos is slow.</b>,
        users.length > 0 ? ` Biggest users: ${memoryUsersPhrase(users)}.` : '',
        ` ${memoryRecommendation(users)}`,
      ];
    }
    case 'unclear': {
      const busiest = busyEnoughToName(episode.busiest_apps);
      return [
        <b>Lucidos is responding slowly.</b>,
        busiest.length > 0 ? ` Busiest apps: ${processorUsersPhrase(busiest)}.` : '',
        ` ${processorRecommendation(episode.busiest_apps)}`,
      ];
    }
  }
}

export function slownessBannerBody(props: {
  layout: BannerLayout;
  episode: SlownessEpisode;
  packaged: boolean;
  onDismiss: () => void;
  elRef?: Ref<HTMLDivElement>;
}): VNode {
  return (
    <div ref={props.elRef} class="slowness-banner" data-layout={props.layout} role="status">
      <SurfaceToneIcon tone="warning" />
      <span class="slowness-banner-text">{slownessSentence(props.episode, props.packaged)}</span>
      <button
        class="icon-btn surface-close slowness-banner-close"
        onClick={props.onDismiss}
        aria-label="Dismiss slowness warning"
        data-tooltip="Dismiss slowness warning"
      >
        <CloseIcon />
      </button>
    </div>
  );
}

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
    packaged: enginePackaged.value === true,
    elRef: ref,
    onDismiss: () => dismissSlownessEpisode(episode.episode_id),
  });
}
