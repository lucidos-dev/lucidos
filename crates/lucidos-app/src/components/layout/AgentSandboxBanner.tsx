import type { Ref, VNode } from 'preact';
import { useRef } from 'preact/hooks';
import { agentSandboxUnavailable } from '../../store/store';
import { currentAgentsUnsandboxed } from '../../store/actions/preferences';
import { openAgentSandboxSettings } from '../../store/actions/menu';
import { viewportIsMobile } from '../../utils/viewport';
import { bannerBelongsToLayout, useBannerHeightVar, type BannerLayout } from './appBanner';
import { SurfaceToneIcon } from '../shared/Surface';

/** The CSS custom property this banner publishes its measured height into. Its
 *  own, not any neighbour's: every bar can be up at once, and
 *  `--app-header-bottom` sums them (see `useBannerHeightVar`). */
export const SANDBOX_BANNER_HEIGHT_VAR = '--app-sandbox-banner-height';

/** What the bar says, or null when agents run sandboxed.
 *
 *  - `unavailable`: agents cannot start at all, a live fault.
 *  - `owner-opted-out`: agents start without the sandbox, a standing risk the
 *    owner chose. It wins, since an opted-out machine refuses nothing. */
export interface SandboxNotice {
  state: 'unavailable' | 'owner-opted-out';
  title: string;
  detail: string;
}

export function agentSandboxNotice(opts: {
  unavailable: string | null;
  optedOut: boolean;
}): SandboxNotice | null {
  if (opts.optedOut) {
    return {
      state: 'owner-opted-out',
      title: 'Agents run unsandboxed.',
      detail: "Any command or coding agent can read Lucidos's own secrets on this machine.",
    };
  }
  if (opts.unavailable !== null) {
    return {
      state: 'unavailable',
      title: 'Agents cannot start here.',
      detail: `This machine cannot sandbox them: ${opts.unavailable}.`,
    };
  }
  return null;
}

/** Pure markup for the bar, hook-free so the tests can invoke it directly.
 *  Not dismissable: the bar retracts when the condition ends, so a dismiss
 *  would only hide it. */
export function agentSandboxBannerBody(props: {
  layout: BannerLayout;
  notice: SandboxNotice | null;
  onOpenSettings: () => void;
  elRef?: Ref<HTMLDivElement>;
}): VNode | null {
  if (!props.notice) return null;
  return (
    <div
      ref={props.elRef}
      class="sandbox-banner"
      data-layout={props.layout}
      data-state={props.notice.state}
      role="status"
    >
      <SurfaceToneIcon tone={props.notice.state === 'unavailable' ? 'error' : 'warning'} />
      <span class="sandbox-banner-text">
        <b>{props.notice.title}</b>{' '}{props.notice.detail}
      </span>
      <button class="action-btn action-btn-secondary" onClick={props.onOpenSettings}>
        Open Permissions
      </button>
    </div>
  );
}

/**
 * The standing word on the agent sandbox (ADR 0645): agents either cannot
 * start on this machine, or the owner runs them without the sandbox.
 *
 * A bar rather than a notification alone, because both conditions persist
 * and both change what every agent can do. The refusal also notifies once.
 *
 * Two mount points, one per layout, per the dual-render rule.
 */
export function AgentSandboxBanner({ layout }: { layout: BannerLayout }) {
  const ref = useRef<HTMLDivElement>(null);
  const notice = agentSandboxNotice({
    unavailable: agentSandboxUnavailable.value,
    optedOut: currentAgentsUnsandboxed(),
  });
  const show = bannerBelongsToLayout(layout, viewportIsMobile.value) && notice !== null;

  useBannerHeightVar(ref, { layout, cssVar: SANDBOX_BANNER_HEIGHT_VAR, active: show });

  if (!show) return null;

  return agentSandboxBannerBody({
    layout,
    notice,
    elRef: ref,
    onOpenSettings: openAgentSandboxSettings,
  });
}
