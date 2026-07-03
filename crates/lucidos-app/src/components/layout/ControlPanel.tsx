import { createPortal } from 'preact/compat';
import { useState, useEffect, useRef } from 'preact/hooks';
import { signal, useSignal } from '@preact/signals';
import { connectionStatus, restartRequired, updateAvailable, engineVersionReady, engineBuilding, enginePackaged, workspaceName } from '../../store/store';
import { initiateEngineRestart } from '../../store/actions/chat-changes';
import { refreshClient } from '../../hooks/sw-update';
import { useLongPress } from '../../hooks/useLongPress';
import { CheckIcon, ReloadIcon } from '../shared/icons';
import { fetchWorkspaces } from '../../api/client';
import type { WorkspaceInfo } from '../../api/client';
import { listWorkspaces, openWorkspace, type WorkspaceStatus } from '../../api/client/control';
import { WORKSPACE_ID, gatewayPickerHref } from '../../utils/basePath';
import type { Loadable } from '../../store/types';
import { toFailed } from '../../store/types';
import { Overlay } from '../shared/Overlay';
import { LoadingFade } from '../shared/LoadingFade';
import { SkeletonProvider, SkText, SkBlock } from '../shared/Skeleton';
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

/** Whether a new engine version is actually READY to switch onto — the honest
 *  "ready for the switch" signal that agrees with the background-build scheme.
 *
 *  In **dev** this is `engineVersionReady` alone: Apply is non-disruptive and
 *  kicks off a background rebuild, so a freshly-applied restart-requiring change
 *  (`restartRequired`) does NOT mean a new version exists yet — the switch only
 *  becomes available once that build finishes and the on-disk binary differs
 *  (the version-status poll flips `engineVersionReady`, see engine-update.ts).
 *  Surfacing it at Apply time lit the badge before anything could have been built.
 *
 *  In **packaged** there is no background build — a newer GitHub release is
 *  immediately installable — so `restartRequired` (set from the outdated-release
 *  check in connection.ts) IS the ready signal; `engineVersionReady` never fires
 *  there (the poll no-ops for packaged builds).
 *
 *  Note: `restartRequired` deliberately still gates the client-refresh ordering
 *  (client-update.ts holds a refresh until after the engine switch, even during
 *  the build window) — that is a different concern from this visible badge. */
export function engineNewVersionReady(): boolean {
  return engineVersionReady.value || (enginePackaged.value && restartRequired.value);
}

/** Visible state of the brand-title badge.
 *  - `building` — a background engine rebuild is in flight (dev): spinning refresh icon.
 *  - `ready` — a new engine version is ready to switch onto, or a newer client
 *    bundle is available to refresh: a single `!` attention mark.
 *  - `none` — nothing to surface; the badge is not rendered.
 *  Building wins over ready: a switch/refresh isn't offered until the build lands. */
export type BrandBadgeState = 'building' | 'ready' | 'none';

export function controlPanelBadgeState(): BrandBadgeState {
  if (engineBuilding.value) return 'building';
  if (engineNewVersionReady() || updateAvailable.value) return 'ready';
  return 'none';
}

export function controlPanelBadgeTooltip(): string | undefined {
  if (engineBuilding.value) return 'Building new version…';
  const newVersion = engineNewVersionReady();
  const update = updateAvailable.value;
  if (newVersion && update) return 'New version available · Client update available';
  if (newVersion) return 'New version available';
  if (update) return 'Client update available';
  return undefined;
}

/** The brand-title badge shared by the desktop + mobile app headers. Renders a
 *  spinning refresh icon while a new engine version builds, a `!` when a
 *  switch/refresh is ready, and nothing otherwise. Reads the driving signals in
 *  its own render so it re-renders in place as they change. */
export function BrandBadge() {
  const state = controlPanelBadgeState();
  if (state === 'none') return null;
  return (
    <span class="badge brand-badge" data-tooltip={controlPanelBadgeTooltip()}>
      {state === 'building'
        ? <span class="brand-badge-spinner"><ReloadIcon /></span>
        : '!'}
    </span>
  );
}

/** Display state for the current workspace's refresh control: its tooltip and
 *  whether to show the client-update dot. Pure so the whole presentation rule is
 *  unit-testable from the three raw signals.
 *
 *  - `ready` = a new engine version is ready to switch onto (`engineNewVersionReady()`)
 *    — a hold restarts onto it; the honest "ready for the switch" signal, NOT the
 *    apply-time `restartRequired`, so the reload glyph only lights once the
 *    background rebuild is ready.
 *  - `clientUpdateAvailable` = a newer client bundle is served (the `updateAvailable`
 *    build-id signal; named without the shadow so the body can't confuse it for
 *    the imported signal).
 *  - `enginePending` = an engine switch is pending or building
 *    (`restartRequired || engineVersionReady`).
 *
 *  The client-refresh dot is advertised only when a newer bundle exists AND no
 *  engine switch is pending/building. With the engine serving a build-pinned
 *  client (api/frontend_snapshot.rs) the web `updateAvailable` signal is already
 *  false during a pending switch (the served build matches the loaded one), so
 *  `!enginePending` is defense-in-depth here — and it also holds the Tauri
 *  app-version axis (which sets `updateAvailable` directly) until after the
 *  switch, so this actionable control never invites a refresh onto the old engine. */
export function currentWorkspaceRefreshState(
  ready: boolean,
  clientUpdateAvailable: boolean,
  enginePending: boolean,
): { tooltip: string; showUpdateBadge: boolean } {
  const update = clientUpdateAvailable && !enginePending;
  const base = ready ? 'Refresh · hold to restart & apply changes' : 'Refresh · hold to restart';
  return { tooltip: update ? `Update available · ${base}` : base, showUpdateBadge: update };
}

/** Whether a click on a Restart-confirm button should actually fire its action.
 *
 *  A long-press reveals the Cancel/Restart confirm buttons UNDER the still-down
 *  pointer, and the browser pairs a stray release `click` with the long-press.
 *  On the mouse path that stray click lands on the freshly-mounted confirm
 *  button — but its `pointerdown` was on the now-unmounted refresh glyph, before
 *  the confirm rendered, so the confirm button never saw a `pointerdown` for it.
 *  A genuine deliberate tap always dispatches its OWN `pointerdown` on the
 *  confirm button first (`freshPointerDown`). Keyboard activation (Enter/Space)
 *  dispatches a `click` with no `pointerdown` at all, so it's allowed via
 *  `keyboard` (a click whose `detail === 0`). Deterministic — no time fuses, no
 *  document listeners. Pure so the "fires on the first click" contract is
 *  directly unit-testable. */
export function shouldActivateConfirm(
  { freshPointerDown, keyboard }: { freshPointerDown: boolean; keyboard: boolean },
): boolean {
  return freshPointerDown || keyboard;
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
  // Gates a confirm-button click on its OWN preceding pointerdown (see
  // shouldActivateConfirm). The stray release click the browser pairs with the
  // long-press has no such pointerdown (it fired on the now-unmounted refresh
  // glyph), so it's ignored deterministically — the user's FIRST deliberate
  // click fires. Reset when the confirm opens and after acting so each reveal
  // starts clean.
  const freshPress = useSignal(false);
  // Highlight the reload glyph as "new version ready to switch onto" only when
  // the rebuild is actually READY (engineNewVersionReady) — not the instant a
  // restart-requiring change is applied. Under the background-build scheme the
  // dev binary is still compiling right after Apply, so keying the highlight off
  // `restartRequired` lit it before there was anything to switch to.
  const ready = engineNewVersionReady();
  // An engine switch is pending or mid-build. currentWorkspaceRefreshState uses
  // this to suppress the client-refresh dot in that window (defense-in-depth atop
  // the build-pinned serving, and the guard for the Tauri app-version axis), so
  // this actionable control never invites a refresh onto the still-old engine.
  const enginePending = restartRequired.value || engineVersionReady.value;

  const longPress = useLongPress(
    () => {
      // The refresh button unmounts as the confirm buttons render in its place
      // under the still-down pointer. Start the reveal with a clean freshPress
      // flag so the stray release click (whose pointerdown was on the refresh
      // glyph, not on a confirm button) is ignored — only a deliberate fresh
      // pointerdown on a confirm button, or a keyboard activation, fires it.
      freshPress.value = false;
      confirming.value = true;
    },
    () => { refreshClient(); },
  );

  // A confirm click acts only when it had its own preceding pointerdown on the
  // button (freshPress) or is keyboard-synthesized (detail === 0). The stray
  // long-press release click has neither, so it no-ops here without a fuse.
  const confirmAllowed = (e: Event): boolean =>
    shouldActivateConfirm({
      freshPointerDown: freshPress.value,
      keyboard: (e as MouseEvent).detail === 0,
    });

  const doRestart = (e: Event) => {
    e.preventDefault();
    e.stopPropagation();
    if (!confirmAllowed(e)) return;
    freshPress.value = false;
    confirming.value = false;
    closeControlPanel();
    void initiateEngineRestart();
  };
  const cancel = (e: Event) => {
    e.preventDefault();
    e.stopPropagation();
    if (!confirmAllowed(e)) return;
    freshPress.value = false;
    confirming.value = false;
  };

  if (confirming.value) {
    const armFresh = () => { freshPress.value = true; };
    return (
      <span class="control-panel-ws-restart-confirm">
        <button type="button" class="control-panel-ws-cancel" onPointerDown={armFresh} onClick={cancel}>Cancel</button>
        <button type="button" class="control-panel-ws-confirm" onPointerDown={armFresh} onClick={doRestart}>Restart</button>
      </span>
    );
  }

  const { tooltip, showUpdateBadge } = currentWorkspaceRefreshState(ready, updateAvailable.value, enginePending);
  return (
    <button
      type="button"
      class={`control-panel-ws-refresh${ready ? ' is-pending' : ''}`}
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

/** Placeholder rows shown while the workspace switcher loads its list. Renders
 *  the real `.control-panel-workspace-row` markup (dot + name) via the shared
 *  self-skeletonizing primitives (Skeleton.tsx → `.sk-bar` leaves), so each row's
 *  height (min-height 2rem) matches a real switcher row exactly — no reflow on
 *  the skeleton→list handoff, and the shimmer vocabulary is shared (no bespoke
 *  skel classes to drift). Decorative → aria-hidden; crossfaded in via
 *  `<LoadingFade>`. */
function WorkspaceSwitcherSkeleton({ rows = 3 }: { rows?: number }) {
  return (
    <SkeletonProvider>
      <div class="control-panel-workspace-skel" aria-hidden="true">
        {Array.from({ length: rows }, (_, i) => (
          <div class="control-panel-workspace-row" key={i}>
            <SkBlock w="0.625rem" h="0.625rem" circle />
            <SkText class="control-panel-ws-name" w="8rem" />
          </div>
        ))}
      </div>
    </SkeletonProvider>
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
