import { createPortal } from 'preact/compat';
import { useState, useEffect, useRef } from 'preact/hooks';
import { signal, useSignal } from '@preact/signals';
import { connectionStatus, restartRequired, updateAvailable, workspaceName } from '../../store/store';
import { initiateEngineRestart } from '../../store/actions/chat-changes';
import { refreshClient } from '../../hooks/sw-update';
import { useLongPress } from '../../hooks/useLongPress';
import { installPairedSwallow } from '../../hooks/useAnchoredPopover';
import { CheckIcon, ReloadIcon } from '../shared/icons';
import { fetchWorkspaces } from '../../api/client';
import type { WorkspaceInfo } from '../../api/client';
import { listWorkspaces, openWorkspace, type WorkspaceStatus } from '../../api/client/control';
import { WORKSPACE_ID, gatewayPickerHref } from '../../utils/basePath';
import type { Loadable } from '../../store/types';
import { toFailed } from '../../store/types';
import { Overlay } from '../shared/Overlay';
import { LoadingFade } from '../shared/LoadingFade';
import { viewportIsMobile } from '../../utils/viewport';

export const controlPanelOpen = signal(false);
/** Anchor element of the toggle that opened the panel. Used for fixed-position
 *  fallback placement and for Overlay's anchor exemption. Set by the brand-label
 *  `data-role="control-panel-toggle"` in AppHeader / MobileAppHeader at open
 *  time. */
export const controlPanelAnchor = signal<HTMLElement | null>(null);
export const controlPanelClickPoint = signal<{ x: number; y: number } | null>(null);

function clampPanelAxis(value: number, size: number, viewportSize: number, margin = 8): number {
  const min = margin;
  const max = Math.max(min, viewportSize - size - margin);
  return Math.max(min, Math.min(value, max));
}

function computePanelPosition(point: { x: number; y: number }, panel: HTMLElement): { top: number; left: number } {
  const width = panel.offsetWidth || panel.getBoundingClientRect().width;
  const height = panel.offsetHeight || panel.getBoundingClientRect().height;
  const gap = 8;
  // Center the popup on the click horizontally; vertically it behaves like a
  // compact dropdown, opening below the pointer unless the viewport needs it above.
  const left = clampPanelAxis(point.x - width / 2, width, window.innerWidth);
  const belowTop = point.y + gap;
  const top = belowTop + height <= window.innerHeight - gap
    ? belowTop
    : point.y - height - gap;
  return {
    top: clampPanelAxis(top, height, window.innerHeight),
    left,
  };
}

function fallbackPointForAnchor(anchor: HTMLElement | null): { x: number; y: number } | null {
  if (!anchor) return null;
  const rect = anchor.getBoundingClientRect();
  return { x: rect.left + rect.width / 2, y: rect.bottom };
}

function samePanelPosition(a: { top: number; left: number } | null, b: { top: number; left: number }): boolean {
  return !!a && a.top === b.top && a.left === b.left;
}

function useMouseAlignedPosition(
  open: boolean,
  point: { x: number; y: number } | null,
  panelRef: { current: HTMLElement | null },
): { top: number; left: number } | null {
  const [pos, setPos] = useState<{ top: number; left: number } | null>(null);

  useEffect(() => {
    if (!open || !point) {
      setPos(null);
      return;
    }
    const panel = panelRef.current;
    if (!panel) return;
    const next = computePanelPosition(point, panel);
    setPos(prev => samePanelPosition(prev, next) ? prev : next);
  });

  useEffect(() => {
    if (!open || !point) return;
    let rafId: number | null = null;
    const recompute = () => {
      rafId = null;
      const panel = panelRef.current;
      if (!panel) return;
      const next = computePanelPosition(point, panel);
      setPos(prev => samePanelPosition(prev, next) ? prev : next);
    };
    const schedule = () => {
      if (rafId !== null) return;
      rafId = requestAnimationFrame(recompute);
    };
    schedule();
    window.addEventListener('scroll', schedule, { capture: true, passive: true });
    window.addEventListener('resize', schedule);
    const vv = window.visualViewport;
    if (vv) {
      vv.addEventListener('resize', schedule);
      vv.addEventListener('scroll', schedule);
    }
    return () => {
      if (rafId !== null) cancelAnimationFrame(rafId);
      window.removeEventListener('scroll', schedule, true);
      window.removeEventListener('resize', schedule);
      if (vv) {
        vv.removeEventListener('resize', schedule);
        vv.removeEventListener('scroll', schedule);
      }
    };
  }, [open, point?.x, point?.y, panelRef]);

  return pos;
}

export function toggleControlPanelAtClick(e: MouseEvent & { currentTarget: EventTarget | null }): void {
  const anchor = e.currentTarget instanceof HTMLElement ? e.currentTarget : null;
  if (!anchor) return;
  const rect = anchor.getBoundingClientRect();
  const hasPointerPosition = e.clientX !== 0 || e.clientY !== 0;
  controlPanelAnchor.value = anchor;
  controlPanelClickPoint.value = hasPointerPosition
    ? { x: e.clientX, y: e.clientY }
    : { x: rect.left + rect.width / 2, y: rect.bottom };
  controlPanelOpen.value = !controlPanelOpen.value;
}

export function controlPanelBadgeCount(): number {
  return (restartRequired.value ? 1 : 0) + (updateAvailable.value ? 1 : 0);
}

export function controlPanelBadgeTooltip(): string | undefined {
  const restart = restartRequired.value;
  const update = updateAvailable.value;
  if (restart && update) return 'Restart needed · Update available';
  if (restart) return 'Restart needed';
  if (update) return 'Update available';
  return undefined;
}

/** Display state for the current workspace's refresh control: its tooltip and
 *  whether to show the update-available dot. Pure so it can be unit-tested across
 *  all four pending×update combinations. `pending` = a restart is pending
 *  (`restartRequired`); `update` = a newer build is available (`updateAvailable`,
 *  the same honest build-id signal that drives the brand badge). */
export function currentWorkspaceRefreshState(
  pending: boolean,
  update: boolean,
): { tooltip: string; showUpdateBadge: boolean } {
  const base = pending ? 'Refresh · hold to restart & apply changes' : 'Refresh · hold to restart';
  return { tooltip: update ? `Update available · ${base}` : base, showUpdateBadge: update };
}

function isGatewayRunning(ws: WorkspaceStatus): boolean {
  return ws.health === 'healthy' || ws.health === 'booting';
}

function gatewayDotClass(ws: WorkspaceStatus): string {
  return ws.health === 'booting' ? 'ws-picker-dot-booting' : 'ws-picker-dot-healthy';
}

function closeControlPanel(): void {
  controlPanelOpen.value = false;
  controlPanelClickPoint.value = null;
}

function ManageWorkspacesItem() {
  // The gateway picker lives at `/~/` (ADR 0014). Behind the gateway that's a
  // relative same-origin route; on a direct engine-port page the gateway is a
  // different origin, so `gatewayPickerHref` builds an absolute URL from the
  // engine-stamped gateway port. `?pick` suppresses the picker's
  // auto-open-last-workspace so this link always lands on the list (see
  // WorkspacePicker `autoOpening`). No href ⇒ no gateway (legacy no-gateway
  // engine) ⇒ no picker to link to ⇒ render nothing.
  const href = gatewayPickerHref();
  if (href === null) return null;
  return (
    <a class="control-panel-workspace-row control-panel-manage-row accent-link" href={href}>
        <span class="control-panel-ws-name">Manage workspaces</span>
    </a>
  );
}

/** Trailing control on the current workspace's row: a refresh button.
 *  A normal tap refreshes the app (reloads the client); a long-press reveals an
 *  inline Restart confirm that restarts this workspace's engine. The confirm is
 *  rendered inside the picker panel on purpose — clicking it neither dismisses
 *  the picker nor gets swallowed by the outside-click contract (a global
 *  confirm modal would be treated as "outside" and eat the click). When a
 *  restart is already pending (changes applied / engine outdated) the button is
 *  highlighted. */
function CurrentWorkspaceControls() {
  const confirming = useSignal(false);
  const pending = restartRequired.value;
  const update = updateAvailable.value;

  const longPress = useLongPress(
    () => {
      confirming.value = true;
      // The refresh button unmounts as the confirm buttons render in its place
      // under the still-down pointer, so useLongPress's own (button-bound) click
      // swallow can't run. This document-level one-shot eats the click/touchend
      // the browser pairs with the long-press release, so it can't activate a
      // confirm button without a deliberate fresh tap.
      installPairedSwallow();
    },
    () => { refreshClient(); },
  );

  const doRestart = (e: Event) => {
    e.preventDefault();
    e.stopPropagation();
    confirming.value = false;
    closeControlPanel();
    void initiateEngineRestart();
  };
  const cancel = (e: Event) => {
    e.preventDefault();
    e.stopPropagation();
    confirming.value = false;
  };

  if (confirming.value) {
    return (
      <span class="control-panel-ws-restart-confirm">
        <button type="button" class="control-panel-ws-cancel" onClick={cancel}>Cancel</button>
        <button type="button" class="control-panel-ws-confirm" onClick={doRestart}>Restart</button>
      </span>
    );
  }

  const { tooltip, showUpdateBadge } = currentWorkspaceRefreshState(pending, update);
  return (
    <button
      type="button"
      class={`control-panel-ws-refresh${pending ? ' is-pending' : ''}`}
      aria-label={tooltip}
      data-tooltip={tooltip}
      onPointerDown={longPress.onPointerDown}
      onPointerMove={longPress.onPointerMove}
      onPointerUp={longPress.onPointerUp}
      onPointerLeave={longPress.onPointerLeave}
      onPointerCancel={longPress.onPointerCancel}
      onContextMenu={longPress.onContextMenu}
      onClick={longPress.onClick}
    >
      <ReloadIcon />
      {/* Non-interactive dot (pointer-events: none) so it never intercepts the
          tap/long-press; mirrors the brand toggle's update badge inside the
          switcher. */}
      {showUpdateBadge && <span class="control-panel-ws-refresh-badge" aria-hidden="true" />}
    </button>
  );
}

/** Shimmer placeholder rows shown while the workspace switcher loads its list.
 *  Reuses `.control-panel-workspace-row` so each row's height (min-height 2rem)
 *  matches a real switcher row exactly — no reflow on the skeleton→list handoff.
 *  Decorative → aria-hidden. Shown immediately (the panel is a popover with no
 *  competing content behind the list — see `.claude/rules/frontend.md`
 *  § "full-screen surface"). */
function WorkspaceSwitcherSkeleton({ rows = 3 }: { rows?: number }) {
  return (
    <div class="control-panel-workspace-skel" aria-hidden="true">
      {Array.from({ length: rows }, (_, i) => (
        <div class="control-panel-workspace-row" key={i}>
          <span class="control-panel-skel-dot" />
          <span class="control-panel-skel-name" />
        </div>
      ))}
    </div>
  );
}

function EmptyItem({ children }: { children: string }) {
  return <div class="control-panel-empty">{children}</div>;
}

export function ControlPanel({ layout }: { layout: 'desktop' | 'mobile' }) {
  const ref = useRef<HTMLDivElement>(null);
  const [wsLoadable, setWsLoadable] = useState<Loadable<WorkspaceInfo[]>>({ status: 'not-loaded' });
  // Gateway switcher (ADR 0014): when a workspace gateway fronts this origin, the
  // control API lists peers addressable as /<slug>/ (same origin). Falls back to
  // the legacy per-port list when the control API isn't reachable (no gateway).
  const [gatewayWs, setGatewayWs] = useState<Loadable<WorkspaceStatus[]>>({ status: 'not-loaded' });
  const open = controlPanelOpen.value;
  // Both AppHeader and MobileAppHeader render simultaneously (dual-layout) and
  // each mounts a ControlPanel. Without this gate, the hidden copy's dismiss
  // hook treats clicks on the visible panel's buttons as "outside" and
  // swallows them.
  const isActiveLayout = layout === (viewportIsMobile.value ? 'mobile' : 'desktop');
  const effectiveOpen = open && isActiveLayout;
  const anchor = controlPanelAnchor.value;
  const clickPoint = controlPanelClickPoint.value ?? fallbackPointForAnchor(anchor);
  const pos = useMouseAlignedPosition(effectiveOpen, clickPoint, ref);

  const status = connectionStatus.value;
  const connected = status === 'connected';

  // Fetch other workspaces on open. Gated to the active layout so the dual-
  // mount doesn't double-fetch on every open. Try the gateway control API first
  // (gateway model); only fall back to the legacy per-port list if there's no
  // gateway (control API unreachable).
  useEffect(() => {
    if (!effectiveOpen) return;
    setGatewayWs({ status: 'loading' });
    listWorkspaces()
      .then(list => setGatewayWs({ status: 'loaded', data: list }))
      .catch(() => {
        setGatewayWs({ status: 'failed', error: 'no gateway' });
        if (!connected) {
          setWsLoadable({ status: 'failed', error: 'disconnected' });
          return;
        }
        setWsLoadable({ status: 'loading' });
        fetchWorkspaces()
          .then(res => setWsLoadable({ status: 'loaded', data: res.workspaces }))
          .catch(e => setWsLoadable(toFailed(e)));
      });
  }, [effectiveOpen, connected]);

  const gatewayPeers =
    gatewayWs.status === 'loaded'
      ? gatewayWs.data.filter(w => isGatewayRunning(w))
      : [];
  const legacyPeers =
    wsLoadable.status === 'loaded'
      ? wsLoadable.data.filter(w => w.engine_running)
      : [];

  // Skeleton is shown while the switcher list is still resolving: gateway not yet
  // tried/loading, or (no-gateway fallback) the legacy list still resolving. Once
  // either path lands (loaded/failed) the real content renders and the skeleton
  // crossfades out via <LoadingFade>. Driving the content branches off the same
  // "resolved" condition keeps the loading frame skeleton-only (nothing peeks
  // through the crossfade).
  const switcherLoading =
    gatewayWs.status === 'not-loaded' ||
    gatewayWs.status === 'loading' ||
    (gatewayWs.status === 'failed' &&
      (wsLoadable.status === 'not-loaded' || wsLoadable.status === 'loading'));

  if (!effectiveOpen) return null;

  return (
    <>
      {typeof document !== 'undefined' && createPortal(<div class="control-panel-scrim" aria-hidden="true" />, document.body)}
      <Overlay
        open
        onClose={closeControlPanel}
        anchor={anchor}
        backdrop={false}
        portal
        panelClass="control-panel"
        panelRef={ref}
        panelStyle={pos
          ? {
              position: 'fixed',
              top: `${pos.top}px`,
              left: `${pos.left}px`,
            }
          : { position: 'fixed', visibility: 'hidden' }}
      >
        <div class="control-panel-workspace-list">
          <LoadingFade showSkeleton={switcherLoading} skeleton={<WorkspaceSwitcherSkeleton />}>
          {gatewayWs.status === 'loaded' && (
            <>
              {gatewayPeers.length === 0 && <EmptyItem>No workspaces running</EmptyItem>}
              {gatewayPeers.map(ws => {
                const active = ws.id === WORKSPACE_ID;
                // The current row is a non-interactive container (not a button)
                // so its refresh control is a real, un-nested <button>.
                if (active) {
                  return (
                    <div
                      class="control-panel-workspace-row is-active control-panel-current-row"
                      key={ws.id}
                      aria-current="page"
                    >
                      <span class={`ws-picker-dot ${gatewayDotClass(ws)}`} />
                      <span class="control-panel-ws-name control-panel-ws-name-current">{ws.name}</span>
                      <CheckIcon className="control-panel-ws-check" />
                      <CurrentWorkspaceControls />
                    </div>
                  );
                }
                return (
                  <button
                    class="control-panel-workspace-row"
                    key={ws.id}
                    onClick={() => openWorkspace(ws.id)}
                  >
                    <span class={`ws-picker-dot ${gatewayDotClass(ws)}`} />
                    <span class="control-panel-ws-name">{ws.name}</span>
                  </button>
                );
              })}
              <ManageWorkspacesItem />
            </>
          )}

          {gatewayWs.status === 'failed' && (wsLoadable.status === 'loaded' || wsLoadable.status === 'failed') && (
            <>
              {wsLoadable.status === 'failed' && (
                <div class="control-panel-empty error-text">
                  {connected ? 'Failed to load workspaces' : 'Workspaces unavailable while disconnected'}
                </div>
              )}
              {wsLoadable.status === 'loaded' && legacyPeers.length === 0 && (
                <EmptyItem>No workspaces running</EmptyItem>
              )}
              {legacyPeers.map(ws => {
                const active = !!workspaceName.value && ws.name === workspaceName.value;
                // The current row is a non-interactive container (not an <a>)
                // so its refresh control is a real, un-nested <button>.
                if (active) {
                  return (
                    <div
                      class="control-panel-workspace-row is-active control-panel-current-row"
                      key={ws.path}
                      aria-current="page"
                    >
                      <span class="ws-picker-dot ws-picker-dot-healthy" />
                      <span class="control-panel-ws-name control-panel-ws-name-current">{ws.name}</span>
                      <CheckIcon className="control-panel-ws-check" />
                      <CurrentWorkspaceControls />
                    </div>
                  );
                }
                return (
                  <a
                    class="control-panel-workspace-row"
                    key={ws.path}
                    // Legacy (no-gateway) fallback: link to the peer engine's own
                    // port on the host the user is already on — never hardcoded
                    // localhost, which would break over Tailscale.
                    href={ws.port ? `${location.protocol}//${location.hostname}:${ws.port}` : undefined}
                    target="_blank"
                    rel="noopener"
                  >
                    <span class="ws-picker-dot ws-picker-dot-healthy" />
                    <span class="control-panel-ws-name">{ws.name}</span>
                    {ws.port && <span class="control-panel-ws-port">:{ws.port}</span>}
                  </a>
                );
              })}
              <ManageWorkspacesItem />
            </>
          )}
          </LoadingFade>
        </div>
      </Overlay>
    </>
  );
}
