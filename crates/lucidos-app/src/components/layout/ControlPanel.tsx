import { createPortal } from 'preact/compat';
import { useState, useEffect, useRef } from 'preact/hooks';
import { signal } from '@preact/signals';
import { connectionStatus, restartRequired, updateAvailable, workspaceName } from '../../store/store';
import { fetchWorkspaces } from '../../api/client';
import type { WorkspaceInfo } from '../../api/client';
import { listWorkspaces, openWorkspace, type WorkspaceStatus } from '../../api/client/control';
import { WORKSPACE_ID } from '../../utils/basePath';
import type { Loadable } from '../../store/types';
import { toFailed } from '../../store/types';
import { Overlay } from '../shared/Overlay';
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
  return (
    <a class="control-panel-workspace-row control-panel-manage-row accent-link" href="/~/">
        <span class="control-panel-ws-name">Manage workspaces</span>
    </a>
  );
}

function LoadingItem() {
  return <div class="control-panel-empty">Loading...</div>;
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
          {gatewayWs.status === 'loading' && <LoadingItem />}

          {gatewayWs.status === 'loaded' && (
            <>
              {gatewayPeers.length === 0 && <EmptyItem>No workspaces running</EmptyItem>}
              {gatewayPeers.map(ws => {
                const active = ws.id === WORKSPACE_ID;
                return (
                  <button
                    class={`control-panel-workspace-row${active ? ' is-active' : ''}`}
                    key={ws.id}
                    aria-current={active ? 'page' : undefined}
                    onClick={() => active ? closeControlPanel() : openWorkspace(ws.id)}
                  >
                    <span class={`ws-picker-dot ${gatewayDotClass(ws)}`} />
                    <span class="control-panel-ws-name">{ws.name}</span>
                    {active && <span class="control-panel-ws-current">Current</span>}
                  </button>
                );
              })}
              <ManageWorkspacesItem />
            </>
          )}

          {gatewayWs.status === 'failed' && (
            <>
              {wsLoadable.status === 'loading' && <LoadingItem />}
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
                return (
                  <a
                    class={`control-panel-workspace-row${active ? ' is-active' : ''}`}
                    key={ws.path}
                    aria-current={active ? 'page' : undefined}
                    href={active ? undefined : ws.port ? `https://localhost:${ws.port}` : undefined}
                    target={active ? undefined : '_blank'}
                    rel={active ? undefined : 'noopener'}
                    onClick={(e) => {
                      if (active) {
                        e.preventDefault();
                        closeControlPanel();
                      }
                    }}
                  >
                    <span class="ws-picker-dot ws-picker-dot-healthy" />
                    <span class="control-panel-ws-name">{ws.name}</span>
                    {active ? <span class="control-panel-ws-current">Current</span> : ws.port && <span class="control-panel-ws-port">:{ws.port}</span>}
                  </a>
                );
              })}
              <ManageWorkspacesItem />
            </>
          )}
        </div>
      </Overlay>
    </>
  );
}
