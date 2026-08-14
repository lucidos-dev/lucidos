/**
 * The in-app workspace switcher: the Lucidos menu's **Workspaces** row and the
 * list it unfolds.
 *
 * Switching used to cost a navigation. The row was an `<a>` to the gateway
 * *workspace picker*, and the plan that made it one
 * (`docs/plans/2026-08-08-lucidos-menu-absorbs-the-workspace-switcher.md`) named
 * the price in its non-goals: "the one-tap hop between two running workspaces
 * goes away". This buys that hop back. It is not a revert of that change: the
 * retired `ControlPanel` was four things at once, three of them (the badge, the
 * Refresh control, the restart guards) already live in the menu and stay there.
 * Only the peer list returns, and it returns INSIDE the menu panel rather than
 * as a second anchored popover with its own placement maths and its own scrim.
 *
 * What it does NOT do is manage workspaces. Create, rename, delete, restore,
 * start, stop, auto-start and Network access all stay on the picker, which is
 * the always-reachable recovery surface; **Manage workspaces** is the list's
 * last row and the way to them. So the two surfaces divide cleanly: the menu
 * switches, the picker manages.
 *
 * A right-click on a row is still switching, into a second window rather than
 * into this one. It unfolds an action row inside the panel, and the picker
 * offers the same thing from its own overflow menu.
 *
 * Three shapes here, in the order they matter:
 *
 * - `workspacesMenuRow` and `workspaceSwitcherList` are PURE functions of their
 *   props, holding no hooks, so every branch (all four `Loadable` states, the
 *   no-gateway row, the current row, an unhealthy one) is unit-testable through
 *   `vnodeToText` with no DOM. Same split as `networkAccessBody`.
 * - `WorkspacesMenuRow` is the hook-bearing wrapper: the expanded flag, the
 *   fetch, the skeleton gate. It is rendered INSIDE the menu's `<Overlay>`, so
 *   closing the menu unmounts it and the next open starts collapsed, with no
 *   stale list to correct. Same trick `WorkspaceRestartRow` uses for its confirm.
 * - `WorkspaceSwitcherSkeleton` mirrors the real row's markup inside a
 *   `SkeletonProvider` (the sanctioned shape for a surface that cannot use
 *   `ListSkeletonOf`'s wrapper, see `.claude/rules/frontend.md`).
 */

import { useSignal } from '@preact/signals';
import { useEffect, useRef } from 'preact/hooks';
import type { Loadable } from '../../store/types';
import { toFailed } from '../../store/types';
import { LoadingFade } from '../shared/LoadingFade';
import { SkeletonProvider, SkText, SkBlock, SkBar } from '../shared/Skeleton';
import { useDelayedFlag } from '../../hooks/useDelayedLoading';
import { CheckIcon, ChevronDownIcon, PopOutIcon } from '../shared/icons';
import { listWorkspaces, openWorkspace, type WorkspaceStatus } from '../../api/client/control';
import { adoptWorkspaceDisplayName } from '../../store/actions/workspace-label';
import { showToast, visibleWorkspaceName } from '../../store/store';
import { offersWorkspaceWindow, openWorkspaceWindow, workspaceWindowLabel } from '../../utils/workspaceWindow';
import { WORKSPACE_ID, gatewayPickerHref } from '../../utils/basePath';
import { rememberLastWorkspaceCount, recallLastWorkspaceCount } from '../../utils/lastWorkspace';
import { workspaceState, workspaceStateLabel } from '../../utils/workspaceState';

/** Skeleton rows when this device has never recorded a workspace count. Two,
 *  not the picker's three: an unfolded list pushes the rows below it down, so
 *  the cheaper guess is the one that moves less when it is wrong. */
const DEFAULT_SKELETON_ROWS = 2;

/** What the placeholder has to draw to stand exactly as tall as the list it
 *  replaces: one row per workspace the last listing saw, PLUS the footer.
 *
 *  The footer is the half that was missing. `workspaceSwitcherList` renders
 *  **Manage workspaces** under every listing that has a picker to reach, so a
 *  placeholder made only of workspace rows was always one row short: the panel
 *  grew by that row at settle and pushed Refresh and Restart down, which is the
 *  bounce the remembered count exists to prevent in the first place.
 *
 *  Pure and exported so the parity is pinned by a test rather than by two call
 *  sites happening to agree about the footer. */
export function skeletonShape(
  remembered: number | null,
  manageHref: string | null,
): { rows: number; manage: boolean } {
  return { rows: remembered ?? DEFAULT_SKELETON_ROWS, manage: manageHref !== null };
}

/** Stacked plates: several of the same thing, one of them on top. Deliberately
 *  not the four-square grid it replaces, which reads as "all apps" and collided
 *  with the sparkle-grid already in the header row. */
function WorkspacesIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
      <path d="M12 2 2 7l10 5 10-5-10-5Z" />
      <path d="m2 12 10 5 10-5" />
      <path d="m2 17 10 5 10-5" />
    </svg>
  );
}

/** The Workspaces row itself, in one of three shapes. Two conditions decide,
 *  and they are NOT the same condition, which is the trap this comment exists
 *  for:
 *
 *  - **`canList`** is whether the control plane can be reached from here. The
 *    control client addresses `/~/api/v1/control/*` as an absolute PATH, so it
 *    resolves to the gateway only while the gateway is this page's origin, i.e.
 *    while we are served under `/<slug>/`. That is the switcher: the row becomes
 *    the expander.
 *  - **`manageHref`** is whether there is a picker to LINK to, which is a weaker
 *    thing: a direct engine-port page is a different origin from the gateway, so
 *    it can still build an absolute URL to the picker out of the stamped
 *    gateway port while every relative control call would hit the engine and
 *    404. That page keeps the row it has always had, a link out.
 *  - Neither: a legacy no-gateway engine. The row STAYS, because it is the only
 *    place either mobile header names the workspace you are in, and renders as a
 *    static label, muted and `pointer-events: none`, honest about having nowhere
 *    to go.
 *
 *  Collapsing the two into one gate is what the browser suite caught: it runs
 *  against exactly the middle case, and an expander there answers a tap with a
 *  404 against control routes the engine does not serve. */
export function workspacesMenuRow({
  canList,
  manageHref,
  workspaceName,
  expanded,
  onToggle,
  onNavigate,
}: {
  canList: boolean;
  manageHref: string | null;
  workspaceName: string | null;
  expanded: boolean;
  onToggle: () => void;
  onNavigate: () => void;
}) {
  // The expander's affordance rides IN the pill, taking the check's slot rather
  // than a slot of its own. The panel's width is a budget tuned so an ordinary
  // workspace name is spelled whole (see --brand-menu-width), and a chevron
  // added beside the pill spends ~15px of it, which is a character and a half:
  // "development" ellipsised the moment one was added. Swapping the marker
  // spends nothing, and it says the more useful thing now that the row leads
  // somewhere: the check meant "you are in dev", and the list it unfolds marks
  // the current workspace with a check of its own anyway.
  const marker = canList
    ? <span class="brand-menu-value-chevron" aria-hidden="true"><ChevronDownIcon /></span>
    : <CheckIcon className="brand-menu-value-check" />;
  // Until the label resolves the pill used to be dropped whole, and that cost
  // more than the name: the expander's chevron rides INSIDE the pill, so the
  // row did not look like an expander at all, and then grew a pill and a marker
  // under the user's finger the moment /health answered. A bar in the name's
  // own slot holds the box and keeps the marker on screen.
  //
  // Drawn immediately, with no `useDelayedFlag` gate, and that is not an
  // exception to the delay rule so much as the case it does not describe: the
  // gate exists so a load STARTED BY THIS RENDER cannot flash a placeholder,
  // and this one started at boot, long before the menu was opened. A gate keyed
  // on the menu opening would just reinstate the blank pill for its first 300ms.
  const name = workspaceName
    ? <span class="brand-menu-value-name">{workspaceName}</span>
    : <span class="brand-menu-value-name"><SkBar w="3.5rem" /></span>;
  const value = (
    <span class="brand-menu-value">
      {!canList && marker}
      {name}
      {canList && marker}
    </span>
  );
  const body = (
    <>
      <WorkspacesIcon />
      Workspaces
      {value}
    </>
  );

  if (canList) {
    return (
      <button
        type="button"
        class={`brand-menu-item brand-menu-ws-toggle${expanded ? ' is-expanded' : ''}`}
        role="menuitem"
        aria-expanded={expanded}
        onClick={onToggle}
      >
        {body}
      </button>
    );
  }

  if (manageHref !== null) {
    return (
      <a class="brand-menu-item" role="menuitem" href={manageHref} onClick={onNavigate}>
        {body}
      </a>
    );
  }

  return (
    <div class="brand-menu-item brand-menu-item-static" role="none">
      {body}
    </div>
  );
}

/** What the unfolded list needs to draw itself. Threaded whole into
 *  {@link switcherRow} rather than unpacked into positional arguments, which at
 *  eight would say nothing at the call site. */
export interface SwitcherListProps {
  state: Loadable<WorkspaceStatus[]>;
  /** The workspace this page is serving, so its row can say so and stay inert. */
  currentId: string | null;
  /** Where the picker is, or null on a client that cannot reach one. */
  manageHref: string | null;
  onSwitch: (w: WorkspaceStatus) => void;
  /** Shut the menu on the way out, as every other menu row does. The document
   *  is about to be replaced either way, so this is about the frame in between:
   *  a menu left open over a page that is loading reads as a tap that missed. */
  onNavigate: () => void;
  /** The workspace whose action row is unfolded, from a right-click on it. At
   *  most one, so a second right-click moves it rather than stacking. */
  contextId: string | null;
  /** A right-click landed on this workspace's row. */
  onContext: (id: string) => void;
  /** The unfolded action row was pressed. */
  onOpenWindow: (w: WorkspaceStatus) => void;
}

/** One workspace. Three shapes, and which one it takes is the whole of this
 *  surface's behaviour:
 *
 *  - **The workspace you are in** is not a navigation target. It carries the
 *    check and `aria-current`, and nothing to press.
 *  - **Unhealthy** is never opened from here. Opening into an unhealthy engine
 *    lands in a dead app shell, which is the reported bug
 *    `WorkspacePicker.openOrRetry` already refuses; the row links to the picker
 *    instead, where Retry lives. With no picker to reach it is inert rather than
 *    a link to nowhere.
 *  - **Everything else** switches. A stopped workspace lazy-starts behind the
 *    gateway's own boot splash, which is the good path rather than the dead one,
 *    and is exactly what tapping its row in the picker does.
 *
 *  A RIGHT-CLICK is a fourth thing, and it crosses all three: it unfolds the
 *  action row below (see {@link switcherActionRow}) instead of raising the
 *  browser's own menu. The current workspace keeps it, since a second window on
 *  the workspace you are in is a real want. An unhealthy one does not, for the
 *  reason its row is not a switch either. */
function switcherRow(w: WorkspaceStatus, props: SwitcherListProps) {
  const { currentId, manageHref, onSwitch, onNavigate, onContext } = props;
  const state = workspaceState(w);
  const stateLabel = workspaceStateLabel(w);
  // aria-label mirrors data-tooltip, as it does in the picker: the dot is the
  // only surface for `last_error`, and a tooltip is hover-only, so without this
  // the error text is unreachable by assistive tech.
  const dot = (
    <span
      class={`ws-picker-dot ws-picker-dot-${state}`}
      data-tooltip={stateLabel}
      aria-label={stateLabel}
    />
  );
  const name = <span class="brand-menu-ws-name">{w.name}</span>;
  const unread = w.unread_count ?? 0;
  const badge = unread > 0 && (
    <span class="brand-menu-ws-badge" aria-label={`${unread} unread notifications`}>
      {unread > 99 ? '99+' : unread}
    </span>
  );

  // Claim the right-click for the action row below. Without `preventDefault`
  // the browser's own menu covers the panel, which is what a right-click on a
  // workspace row does today. Nothing else is suppressed: a right-click
  // dispatches no `click`, so the row's switch was never going to run.
  const contextMenu = offersWorkspaceWindow(state)
    ? { onContextMenu: (e: MouseEvent) => { e.preventDefault(); onContext(w.id); } }
    : {};

  // A `menuitem` that is deliberately not actionable, NOT `role="none"`: the
  // two inert rows carry global ARIA (`aria-current`, `aria-label`), and ARIA's
  // conflict resolution drops a presentation role from any element that does,
  // so the pair would be self-contradictory. `aria-disabled` states the same
  // thing without lying about what the row is, and keeps the panel's owned
  // elements all menuitems.
  if (w.id === currentId) {
    return (
      <div
        class="brand-menu-ws-row is-current"
        role="menuitem"
        aria-disabled="true"
        aria-current="page"
        key={w.id}
        {...contextMenu}
      >
        {dot}
        {name}
        {badge}
        <CheckIcon className="brand-menu-ws-check" />
      </div>
    );
  }

  if (state === 'unhealthy') {
    const hint = `${stateLabel} · retry it in the workspace picker`;
    if (manageHref === null) {
      return (
        <div
          class="brand-menu-ws-row is-unreachable"
          role="menuitem"
          aria-disabled="true"
          data-tooltip={hint}
          aria-label={`${w.name} · ${hint}`}
          key={w.id}
        >
          {dot}
          {name}
        </div>
      );
    }
    return (
      <a
        class="brand-menu-ws-row is-unreachable"
        role="menuitem"
        href={manageHref}
        onClick={onNavigate}
        data-tooltip={hint}
        aria-label={`${w.name} · ${hint}`}
        key={w.id}
      >
        {dot}
        {name}
      </a>
    );
  }

  return (
    <button
      type="button"
      class="brand-menu-ws-row"
      role="menuitem"
      aria-label={`Switch to ${w.name} · ${stateLabel}`}
      onClick={() => onSwitch(w)}
      key={w.id}
      {...contextMenu}
    >
      {dot}
      {name}
      {badge}
    </button>
  );
}

/** The action a right-click unfolds, as a row of the menu rather than as a
 *  popover over it.
 *
 *  Two independent reasons it is not a nested `<Overlay>`. The panel is
 *  `transform`ed and `overflow: hidden auto`, so a `position: fixed` child
 *  resolves against it and is clipped by it. A portaled panel is also OUTSIDE
 *  this menu for the dismiss contract. The first pointerdown on it would
 *  therefore shut the menu and unmount the action with it.
 *
 *  `WorkspaceRestartRow` renders its confirm inline for the second reason. The
 *  panel's own list already unfolds this way too, so this is the shape the
 *  surface has rather than a workaround. */
function switcherActionRow(w: WorkspaceStatus, onOpenWindow: (w: WorkspaceStatus) => void) {
  const label = workspaceWindowLabel();
  return (
    <button
      type="button"
      class="brand-menu-ws-row brand-menu-ws-action"
      role="menuitem"
      // Names the workspace, which the visible label cannot: the row sits under
      // it, and an `aria-label` replaces the content a screen reader would read.
      aria-label={`${label}: ${w.name}`}
      onClick={() => onOpenWindow(w)}
      // A colon, because a slug cannot contain one and a workspace row's key is
      // a bare slug. `${w.id}-window` could not promise that: it collides with
      // the row of a workspace actually named `<id>-window`, and two siblings
      // sharing a key is how keyed diffing reuses the wrong node.
      key={`window:${w.id}`}
    >
      <PopOutIcon />
      <span class="brand-menu-ws-name">{label}</span>
    </button>
  );
}

/** The unfolded list. Pure, so all four `Loadable` states are unit-testable.
 *
 *  `not-loaded` / `loading` render nothing at all: the skeleton the caller
 *  crossfades in owns that frame, and content peeking through it is what makes a
 *  crossfade look like a flicker. A `failed` load renders as a failure and NOT
 *  as an empty list, and keeps the Manage row, so a gateway that dropped while
 *  the menu was open still leaves a way out. */
export function workspaceSwitcherList(props: SwitcherListProps) {
  const { state, manageHref, contextId, onNavigate, onOpenWindow } = props;
  if (state.status === 'not-loaded' || state.status === 'loading') return null;
  return (
    <div class="brand-menu-ws-list" role="group" aria-label="Switch workspace">
      {state.status === 'failed' && (
        <p class="brand-menu-ws-note brand-menu-ws-error">
          Could not list workspaces: {state.error}
        </p>
      )}
      {/* The action row is emitted right under the workspace it belongs to,
          rather than at the end of the list, so it cannot be read as belonging
          to whichever row it happened to land beside.

          `offersWorkspaceWindow` is asked again here, and not only where the
          right-click is claimed. `contextId` and `state` arrive as independent
          props, so this function cannot assume its caller paired them: a row
          may only carry the action while its own state still justifies one. */}
      {state.status === 'loaded' &&
        state.data.flatMap((w) => [
          switcherRow(w, props),
          w.id === contextId && offersWorkspaceWindow(workspaceState(w))
            ? switcherActionRow(w, onOpenWindow)
            : null,
        ])}
      {manageHref !== null && (
        <a
          class="brand-menu-ws-row brand-menu-ws-manage"
          role="menuitem"
          href={manageHref}
          onClick={onNavigate}
        >
          Manage workspaces
        </a>
      )}
    </div>
  );
}

/** Placeholder rows while the list loads: the real row's markup (dot, name)
 *  inside a `SkeletonProvider`, so a row's height mirrors a loaded one by
 *  construction and the handoff does not reflow. Decorative, so aria-hidden.
 *  Both counts come from {@link skeletonShape}, which owns why the footer is
 *  drawn at all. */
function WorkspaceSwitcherSkeleton({ rows, manage }: { rows: number; manage: boolean }) {
  return (
    <SkeletonProvider>
      <div class="brand-menu-ws-list" aria-hidden="true">
        {Array.from({ length: rows }, (_, i) => (
          <div class="brand-menu-ws-row" key={i}>
            {/* The `.ws-picker-dot` footprint, so a shimmer row is exactly as
                tall and as indented as a loaded one. */}
            <SkBlock w="0.625rem" h="0.625rem" circle />
            <SkText class="brand-menu-ws-name" w="7rem" />
          </div>
        ))}
        {/* The Manage workspaces footer: both of its classes, so it takes the
            smaller type and carries no dot, exactly as the loaded list draws
            it. */}
        {manage && (
          <div class="brand-menu-ws-row brand-menu-ws-manage">
            <SkText w="8rem" />
          </div>
        )}
      </div>
    </SkeletonProvider>
  );
}

/**
 * The Workspaces row plus the list it unfolds.
 *
 * The fetch hangs off the EXPAND, never off the menu opening: Refresh and
 * Restart have nothing to do with workspaces, and a control-plane request on
 * every menu open would put gateway traffic behind both of them. Every expand
 * refetches, for the reason the picker's popover documents: what is shown must
 * be what the gateway says now, not what it said the last time it was asked.
 *
 * `onClose` shuts the menu on a switch. The navigation is a full document load,
 * so the current page stays on screen until the next one paints; leaving the
 * menu sitting over it would read as a tap that did nothing.
 */
export function WorkspacesMenuRow({ onClose }: { onClose: () => void }) {
  const manageHref = gatewayPickerHref();
  // Served under `/<slug>/`, so the gateway is this page's origin and the
  // control client's absolute `/~/…` path reaches it. See `workspacesMenuRow`
  // for why this is not the same question as "is there a picker to link to".
  const canList = WORKSPACE_ID !== null;
  const expanded = useSignal(false);
  const list = useSignal<Loadable<WorkspaceStatus[]>>({ status: 'not-loaded' });
  // Sized to the count the last successful listing saw, on this device, so the
  // skeleton does not bounce into the list. Captured once per mount so it holds
  // still while the skeleton fades out over the fresher count just recorded.
  // `null` is "nothing recorded", which is a real answer and must survive the
  // `undefined` sentinel that means "not captured yet".
  const remembered = useRef<number | null | undefined>(undefined);
  if (remembered.current === undefined) remembered.current = recallLastWorkspaceCount();
  const skeleton = skeletonShape(remembered.current, manageHref);
  // Which workspace has its action row unfolded, from a right-click on it.
  // Cleared by every fetch, so a row that left the registry since the last one
  // cannot keep an action unfolded on it. Closing the menu unmounts this whole
  // component, so a fresh open starts with nothing unfolded either.
  const contextId = useSignal<string | null>(null);

  useEffect(() => {
    // Never fires in the two non-switcher shapes: they cannot expand, so the
    // control request the engine would 404 is never made.
    if (!expanded.value) return;
    let live = true;
    list.value = { status: 'loading' };
    contextId.value = null;
    listWorkspaces()
      .then((data) => {
        if (!live) return;
        list.value = { status: 'loaded', data };
        // The same listing the rows render, so the pill above them cannot show a
        // pre-rename name while the list shows the new one. This is the second
        // adopter beside the boot probe (see store/actions/workspace-label.ts).
        adoptWorkspaceDisplayName(data);
        // Keep the picker's own skeleton honest too: the count is device-global
        // and either surface may be the one that last saw the list.
        rememberLastWorkspaceCount(data.length);
      })
      .catch((e) => {
        if (!live) return;
        list.value = toFailed(e);
      });
    return () => { live = false; };
  }, [expanded.value]);

  const loading = list.value.status === 'not-loaded' || list.value.status === 'loading';
  const showSkeleton = useDelayedFlag(expanded.value && loading);

  return (
    <>
      {workspacesMenuRow({
        canList,
        manageHref,
        workspaceName: visibleWorkspaceName.value,
        expanded: expanded.value,
        onToggle: () => { expanded.value = !expanded.value; },
        onNavigate: onClose,
      })}
      {expanded.value && (
        <LoadingFade
          showSkeleton={showSkeleton}
          skeleton={<WorkspaceSwitcherSkeleton rows={skeleton.rows} manage={skeleton.manage} />}
        >
          {workspaceSwitcherList({
            state: list.value,
            currentId: WORKSPACE_ID,
            manageHref,
            onSwitch: (w) => { onClose(); openWorkspace(w.id); },
            onNavigate: onClose,
            contextId: contextId.value,
            onContext: (id) => { contextId.value = id; },
            // Shuts the menu, as every other action row does. This page is not
            // replaced (the workspace opens elsewhere), so the menu would
            // otherwise sit there over a job already done.
            //
            // The toast names the workspace, and it is the only report the user
            // gets: this is a direct click, so the telemetry carve-out in
            // `.claude/rules/frontend.md` does not cover it.
            onOpenWindow: (w) => {
              contextId.value = null;
              onClose();
              void openWorkspaceWindow(w.id).catch((e: unknown) => {
                showToast(`Could not open ${w.name}: ${e}`, 'error');
              });
            },
          })}
        </LoadingFade>
      )}
    </>
  );
}
