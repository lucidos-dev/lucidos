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
 * WHERE a row opens is not decided here. `utils/workspaceWindow.ts` owns that,
 * so the picker cannot disagree: a plain activation takes this client's default
 * mode, and a right-click unfolds the alternate as a row inside the panel. The
 * picker offers the same alternate from its own overflow menu.
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
import { SkeletonProvider, SkText, SkBlock } from '../shared/Skeleton';
import { useDelayedFlag } from '../../hooks/useDelayedLoading';
import { CheckIcon, ChevronDownIcon, PopInIcon, PopOutIcon } from '../shared/icons';
import { listWorkspaces, type WorkspaceStatus } from '../../api/client/control';
import { adoptWorkspaceDisplayName } from '../../store/actions/workspace-label';
import { showToast, visibleWorkspaceName } from '../../store/store';
import {
  alternateOpenMode,
  middleClickActivates,
  middleClickHandler,
  openModeForClick,
  openModeLabel,
  openWorkspaceIn,
  type WorkspaceOpenMode,
} from '../../utils/workspaceWindow';
import { WORKSPACE_ID, gatewayPickerHref } from '../../utils/basePath';
import { replaceOnPlainClick } from '../../utils/documentNavigation';
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
  // The pill renders whether or not the label has resolved. Dropping it whole
  // dropped the marker with it, so the row carried no marker until /health
  // answered, and then grew both at once.
  //
  // What is held is the MARKER, and the empty name slot draws nothing: no
  // placeholder, no shimmer. See `.claude/rules/frontend.md` on a single value
  // inside real markup.
  //
  // None is owed, but only because the marker TRAILS the name. The pill is
  // `margin-left: auto`, so it is anchored on the right and grows leftward. A
  // leading marker would be dragged left by the whole name when it lands, which
  // is the shift this avoids. Trailing also puts the check on the side the
  // unfolded list gives the current workspace's check. One rule in
  // header-mark.css covers both, being the same mark doing the same job.
  const value = (
    <span class="brand-menu-value">
      {workspaceName && <span class="brand-menu-value-name">{workspaceName}</span>}
      {marker}
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
      <a
        class="brand-menu-item"
        role="menuitem"
        href={manageHref}
        onClick={replaceOnPlainClick(manageHref, onNavigate)}
      >
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
  /** A row was activated. The event decides the mode, so a cmd-click or a
   *  middle-click can ask for a tab where a plain click switches in place. */
  onActivate: (w: WorkspaceStatus, e: MouseEvent) => void;
  /** Shut the menu on the way out, as every other menu row does. The document
   *  is about to be replaced either way, so this is about the frame in between:
   *  a menu left open over a page that is loading reads as a tap that missed. */
  onNavigate: () => void;
  /** The workspace whose action row is unfolded, from a right-click on it. At
   *  most one, so a second right-click moves it rather than stacking. */
  contextId: string | null;
  /** A right-click landed on this workspace's row. */
  onContext: (id: string) => void;
  /** The unfolded action row was pressed, in the mode it offered. */
  onAlternate: (w: WorkspaceStatus, mode: WorkspaceOpenMode) => void;
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
 *  - **Everything else** opens, in whatever mode this client makes the default.
 *    A stopped workspace lazy-starts behind the gateway's own boot splash, which
 *    is the good path rather than the dead one. That is exactly what tapping its
 *    row in the picker does.
 *
 *  A RIGHT-CLICK is a fourth thing, and it crosses all three: it unfolds the
 *  action row below (see {@link switcherActionRow}) instead of raising the
 *  browser's own menu. Which rows carry it is `alternateOpenMode`'s decision,
 *  so the picker cannot quietly disagree. */
function switcherRow(w: WorkspaceStatus, props: SwitcherListProps) {
  const { currentId, manageHref, onActivate, onNavigate, onContext } = props;
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
  // dispatches no `click`, so the row's own action was never going to run.
  //
  // Claimed for every row an action row could ever open under, NOT only the
  // rows that have one right now. Tying the two together left one row whose
  // right-click raised the webview's own menu over the panel, which is the
  // very thing this suppresses. Unfolding stays `alternateOpenMode`'s call.
  const contextMenu = state !== 'unhealthy'
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
        onClick={replaceOnPlainClick(manageHref, onNavigate)}
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
      onClick={(e: MouseEvent) => onActivate(w, e)}
      onAuxClick={middleClickActivates(state) ? middleClickHandler((e) => onActivate(w, e)) : undefined}
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
function switcherActionRow(
  w: WorkspaceStatus,
  mode: WorkspaceOpenMode,
  onAlternate: (w: WorkspaceStatus, mode: WorkspaceOpenMode) => void,
) {
  const label = openModeLabel(mode);
  return (
    <button
      type="button"
      class="brand-menu-ws-row brand-menu-ws-action"
      role="menuitem"
      // Names the workspace, which the visible label cannot: the row sits under
      // it, and an `aria-label` replaces the content a screen reader would read.
      aria-label={`${label}: ${w.name}`}
      onClick={() => onAlternate(w, mode)}
      // A colon, because a slug cannot contain one and a workspace row's key is
      // a bare slug. `${w.id}-window` could not promise that: it collides with
      // the row of a workspace actually named `<id>-window`, and two siblings
      // sharing a key is how keyed diffing reuses the wrong node.
      key={`window:${w.id}`}
    >
      {mode === 'separate' ? <PopOutIcon /> : <PopInIcon />}
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
  const { state, currentId, manageHref, contextId, onNavigate, onAlternate } = props;
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

          `alternateOpenMode` is asked again here, and not only where the
          right-click is claimed. `contextId` and `state` arrive as independent
          props, so this function cannot assume its caller paired them: a row
          may only carry the action while its own state still justifies one. */}
      {state.status === 'loaded' &&
        state.data.flatMap((w) => {
          const mode = alternateOpenMode(workspaceState(w), currentId, w.id);
          return [
            switcherRow(w, props),
            w.id === contextId && mode !== null
              ? switcherActionRow(w, mode, onAlternate)
              : null,
          ];
        })}
      {manageHref !== null && (
        <a
          class="brand-menu-ws-row brand-menu-ws-manage"
          role="menuitem"
          href={manageHref}
          onClick={replaceOnPlainClick(manageHref, onNavigate)}
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

  // Every way out of this list, in either mode. The menu always shuts. An
  // in-place switch replaces the document, and a menu left over the loading
  // page reads as a tap that missed. A separate one happens elsewhere, so the
  // menu would sit over work already done.
  //
  // The toast names the workspace, and it is the only report the user gets.
  // This is a direct click, so the telemetry carve-out in
  // `.claude/rules/frontend.md` does not cover it.
  const open = (w: WorkspaceStatus, mode: WorkspaceOpenMode) => {
    contextId.value = null;
    onClose();
    void openWorkspaceIn(mode, w.id).catch((e: unknown) => {
      showToast(`Could not open ${w.name}: ${e}`, 'error');
    });
  };

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
            // A null mode is a gesture that is not an activation at all (a
            // macOS Ctrl-click, which the action row already answers).
            onActivate: (w, e) => {
              const mode = openModeForClick(e);
              if (mode !== null) open(w, mode);
            },
            onNavigate: onClose,
            contextId: contextId.value,
            onContext: (id) => { contextId.value = id; },
            onAlternate: open,
          })}
        </LoadingFade>
      )}
    </>
  );
}
