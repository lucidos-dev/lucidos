import type { Ref, VNode } from 'preact';
import { useEffect, useRef, useState } from 'preact/hooks';
import { startThisWorkspace } from '../../store/actions/workspace-start';
import { connectionNoticeStatus, visibleWorkspaceName } from '../../store/store';
import { remoteConnectionFailure } from '../../utils/remoteConnection';
import { REMOTE_GATEWAY, WORKSPACE_ID } from '../../utils/basePath';
import { connectionNotice, hasConnectionNotice, type ConnectionNoticeStatus } from '../../utils/connectionNotice';
import { viewportIsMobile } from '../../utils/viewport';
import { useDelayedFlag } from '../../hooks/useDelayedLoading';
import { bannerBelongsToLayout, useBannerHeightVar, type BannerLayout } from './appBanner';

export const CONNECTION_BANNER_HEIGHT_VAR = '--app-conn-banner-height';

export const CONNECTING_QUIET_MS = 8000;

export function shouldRenderConnectionBanner(opts: {
  layout: BannerLayout;
  mobileViewport: boolean;
  status: ConnectionNoticeStatus;
  connectingSettled: boolean;
}): boolean {
  if (!bannerBelongsToLayout(opts.layout, opts.mobileViewport)) return false;
  if (!hasConnectionNotice(opts.status)) return false;
  return opts.status === 'connecting' ? opts.connectingSettled : true;
}

export function connectionBannerBody(props: {
  layout: BannerLayout;
  status: ConnectionNoticeStatus;
  workspace: string | null;
  onStart?: () => void;
  starting?: boolean;
  elRef?: Ref<HTMLDivElement>;
}): VNode | null {
  const notice = connectionNotice(props.status, props.workspace, 'full');
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
      {props.status === 'stopped' && props.onStart && (
        <button class="action-btn" onClick={props.onStart} disabled={props.starting}>
          {props.starting ? 'Starting…' : 'Start'}
        </button>
      )}
    </div>
  );
}

export function ConnectionBanner({ layout }: { layout: BannerLayout }) {
  const ref = useRef<HTMLDivElement>(null);
  const status = connectionNoticeStatus.value;
  const connectingSettled = useDelayedFlag(status === 'connecting', CONNECTING_QUIET_MS);
  const [starting, setStarting] = useState(false);
  useEffect(() => {
    if (status !== 'stopped') setStarting(false);
  }, [status]);
  const remoteFailure = remoteConnectionFailure.value;
  const show = (REMOTE_GATEWAY !== null && remoteFailure !== null && bannerBelongsToLayout(layout, viewportIsMobile.value)) || shouldRenderConnectionBanner({
    layout,
    mobileViewport: viewportIsMobile.value,
    status,
    connectingSettled,
  });

  useBannerHeightVar(ref, { layout, cssVar: CONNECTION_BANNER_HEIGHT_VAR, active: show });

  if (!show) return null;

  if (REMOTE_GATEWAY && remoteFailure) return <div ref={ref} class="connection-banner" data-layout={layout} role="status">
    <span>Cannot reach @{REMOTE_GATEWAY}. {remoteFailure.reason}</span>
    <a href="/~/?pick">Choose a workspace</a>
  </div>;

  const workspaceId = REMOTE_GATEWAY ? null : WORKSPACE_ID;
  const onStart = workspaceId === null ? undefined : () => {
    if (starting) return;
    setStarting(true);
    void startThisWorkspace(workspaceId).then((started) => {
      if (!started) setStarting(false);
    });
  };

  return connectionBannerBody({
    layout,
    status,
    workspace: visibleWorkspaceName.value,
    onStart,
    starting,
    elRef: ref,
  });
}
