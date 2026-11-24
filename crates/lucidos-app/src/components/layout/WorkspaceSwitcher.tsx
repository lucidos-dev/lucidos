
import { useSignal } from '@preact/signals';
import { useEffect, useRef } from 'preact/hooks';
import type { Loadable } from '../../store/types';
import { toFailed } from '../../store/types';
import { LoadingFade } from '../shared/LoadingFade';
import { Disclosure } from '../shared/Disclosure';
import { SkeletonProvider, SkText, SkBlock } from '../shared/Skeleton';
import { useDelayedFlag } from '../../hooks/useDelayedLoading';
import { CheckIcon, ChevronDownIcon } from '../shared/icons';
import { listRemoteWorkspaces, listWorkspacesWithOrder, type WorkspaceStatus } from '../../api/client/control';
import { adoptWorkspaceDisplayName } from '../../store/actions/workspace-label';
import { showToast, visibleWorkspaceName } from '../../store/store';
import {
  alternateOpenMode,
  middleClickActivates,
  middleClickHandler,
  openModeForClick,
  openWorkspaceIn,
  type WorkspaceOpenMode,
} from '../../utils/workspaceWindow';
import { workspaceActionRow } from './WorkspaceActionRow';
import { WORKSPACE_ID, gatewayPickerHref } from '../../utils/basePath';
import { errorDetail } from '../../utils/errorDetail';
import { replaceOnPlainClick } from '../../utils/documentNavigation';
import {
  rememberLastWorkspaceCount,
  recallLastWorkspaceCount,
  rememberWorkspaceSwitcherExpanded,
  recallWorkspaceSwitcherExpanded,
} from '../../utils/lastWorkspace';
import { LoadableError } from '../shared/LoadableError';
import {
  mergeRemoteListings,
  recallRemoteListings,
  rememberRemoteListings,
  remoteWorkspaceOpenable,
  type RemoteGatewayView,
} from '../../utils/remoteWorkspaces';
import { orderWorkspaces, type OrderedWorkspace } from '../../utils/workspaceOrder';
import { REMOTE_GATEWAY } from '../../utils/basePath';
import { workspaceState, workspaceStateLabel } from '../../utils/workspaceState';

const DEFAULT_SKELETON_ROWS = 2;

const SWITCHER_ACTION_INDENT = 'brand-menu-ws-action-under-dot';

export function skeletonShape(
  remembered: number | null,
  remoteRows: number,
  manageHref: string | null,
): { rows: number; manage: boolean } {
  return { rows: (remembered ?? DEFAULT_SKELETON_ROWS) + remoteRows, manage: manageHref !== null };
}

export function initiallyExpanded(canList: boolean, remembered: boolean): boolean {
  return canList && remembered;
}

function WorkspacesIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
      <path d="M12 2 2 7l10 5 10-5-10-5Z" />
      <path d="m2 12 10 5 10-5" />
      <path d="m2 17 10 5 10-5" />
    </svg>
  );
}

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
  const marker = canList
    ? <span class="brand-menu-value-chevron" aria-hidden="true"><ChevronDownIcon /></span>
    : <CheckIcon className="brand-menu-value-check" />;
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

export interface SwitcherListProps {
  state: Loadable<WorkspaceStatus[]>;
  currentId: string | null;
  manageHref: string | null;
  onActivate: (w: WorkspaceStatus, e: MouseEvent) => void;
  onNavigate: () => void;
  contextId: string | null;
  onContext: (id: string) => void;
  onAlternate: (w: WorkspaceStatus, mode: WorkspaceOpenMode) => void;
  remote: readonly RemoteGatewayView[];
  remoteError: string | null;
  order: readonly string[];
  onOpenRemote: (route: string, mode: WorkspaceOpenMode) => void;
}

type RemoteSwitcherEntry = Extract<OrderedWorkspace<WorkspaceStatus>, { kind: 'remote' }>;

const NO_LOCAL_WORKSPACES: readonly WorkspaceStatus[] = [];

function remoteShutReason(view: RemoteGatewayView): string {
  if (view.unreachable === null) return 'Not running. Start it on the remote gateway.';
  const lastSeen = view.lastSeenAt ? ` · Last answer ${new Date(view.lastSeenAt).toLocaleString()}` : '';
  return `${view.unreachable}${lastSeen}`;
}

function remoteSwitcherRow({ key: route, workspace: w, view }: RemoteSwitcherEntry, props: SwitcherListProps) {
  const { currentId, onOpenRemote } = props;
  const state = workspaceState(w);
  const reachable = remoteWorkspaceOpenable(view, w);
  const status = reachable ? workspaceStateLabel(w) : remoteShutReason(view);
  const body = (
    <>
      <span class={`ws-picker-dot ws-picker-dot-${state}`} data-tooltip={status} aria-hidden="true" />
      <span class="brand-menu-ws-name">
        {w.name} <span class="brand-menu-ws-gateway">@{view.gateway}</span>
      </span>
    </>
  );

  if (route === currentId) {
    return (
      <div class="brand-menu-ws-row is-current" role="menuitem" aria-disabled="true" aria-current="page" key={route}>
        {body}
        <CheckIcon className="brand-menu-ws-check" />
      </div>
    );
  }

  if (!reachable) {
    return (
      <div
        class="brand-menu-ws-row is-shut"
        role="menuitem"
        aria-disabled="true"
        data-tooltip={status}
        aria-label={`${w.name} on ${view.gateway} · ${status}`}
        key={route}
      >
        {body}
      </div>
    );
  }

  const activate = (e: MouseEvent) => {
    e.preventDefault();
    const mode = openModeForClick(e);
    if (mode !== null) onOpenRemote(route, mode);
  };
  return (
    <a
      class="brand-menu-ws-row"
      role="menuitem"
      href={`/${route}/`}
      aria-label={`Switch to ${w.name} on ${view.gateway} · ${status}`}
      onClick={activate}
      onAuxClick={middleClickActivates(state) ? middleClickHandler(activate) : undefined}
      key={route}
    >
      {body}
    </a>
  );
}

function switcherRow(w: WorkspaceStatus, props: SwitcherListProps) {
  const { currentId, manageHref, onActivate, onNavigate, onContext } = props;
  const state = workspaceState(w);
  const stateLabel = workspaceStateLabel(w);
  const dot = (
    <span
      class={`ws-picker-dot ws-picker-dot-${state}`}
      data-tooltip={stateLabel}
      aria-label={stateLabel}
    />
  );
  const name = <span class="brand-menu-ws-name">{w.name}</span>;

  const contextMenu = state !== 'unhealthy'
    ? { onContextMenu: (e: MouseEvent) => { e.preventDefault(); onContext(w.id); } }
    : {};

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
    </button>
  );
}

export function workspaceSwitcherList(props: SwitcherListProps) {
  const { state, currentId, manageHref, contextId, onNavigate, onAlternate, remote, remoteError, order } = props;
  if (state.status === 'not-loaded' || state.status === 'loading') return null;
  return (
    <div class="brand-menu-ws-list" role="group" aria-label="Switch workspace">
      <div class="brand-menu-ws-scroll">
        {state.status === 'failed' && (
          <p class="brand-menu-ws-note brand-menu-ws-error">
            Could not list workspaces: {state.error}
          </p>
        )}
        {orderWorkspaces(state.status === 'loaded' ? state.data : NO_LOCAL_WORKSPACES, remote, order).flatMap((entry) => {
          if (entry.kind === 'remote') return [remoteSwitcherRow(entry, props)];
          const w = entry.workspace;
          const mode = alternateOpenMode(workspaceState(w), currentId, w.id);
          return [
            switcherRow(w, props),
            workspaceActionRow({
              open: w.id === contextId,
              id: w.id,
              name: w.name,
              mode,
              indentClass: SWITCHER_ACTION_INDENT,
              onActivate: (m) => onAlternate(w, m),
            }),
          ];
        })}
        {remoteError !== null && (
          <div role="alert"><LoadableError error={remoteError} noun="remote workspaces" /></div>
        )}
      </div>
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

function WorkspaceSwitcherSkeleton({ rows, manage }: { rows: number; manage: boolean }) {
  return (
    <SkeletonProvider>
      <div class="brand-menu-ws-list" aria-hidden="true">
        <div class="brand-menu-ws-scroll">
          {Array.from({ length: rows }, (_, i) => (
            <div class="brand-menu-ws-row" key={i}>
              <SkBlock w="0.625rem" h="0.625rem" circle />
              <SkText class="brand-menu-ws-name" w="7rem" />
            </div>
          ))}
        </div>
        {manage && (
          <div class="brand-menu-ws-row brand-menu-ws-manage">
            <SkText w="8rem" />
          </div>
        )}
      </div>
    </SkeletonProvider>
  );
}

export function WorkspacesMenuRow({ onClose }: { onClose: () => void }) {
  const manageHref = gatewayPickerHref();
  const canList = WORKSPACE_ID !== null;
  const expanded = useSignal(initiallyExpanded(canList, recallWorkspaceSwitcherExpanded()));
  const list = useSignal<Loadable<WorkspaceStatus[]>>({ status: 'not-loaded' });
  const order = useSignal<readonly string[]>([]);
  const remote = useSignal<Loadable<RemoteGatewayView[]>>({ status: 'not-loaded' });
  const cachedRemote = useRef(recallRemoteListings());
  const remembered = useRef<number | null | undefined>(undefined);
  if (remembered.current === undefined) remembered.current = recallLastWorkspaceCount();
  const skeleton = skeletonShape(
    remembered.current,
    cachedRemote.current.reduce((n, view) => n + view.workspaces.length, 0),
    manageHref,
  );
  const contextId = useSignal<string | null>(null);

  useEffect(() => {
    if (!expanded.value) return;
    let live = true;
    list.value = { status: 'loading' };
    contextId.value = null;
    cachedRemote.current = cachedRemote.current.map((view) => ({ ...view, unreachable: 'Checking gateway' }));
    remote.value = { status: 'loading' };
    listRemoteWorkspaces().then((listings) => {
      if (!live) return;
      const data = mergeRemoteListings(cachedRemote.current, listings);
      cachedRemote.current = data;
      rememberRemoteListings(data);
      remote.value = { status: 'loaded', data };
    }).catch((e) => {
      if (!live) return;
      const failure = { status: 'failed' as const, error: errorDetail(e) };
      cachedRemote.current = cachedRemote.current.map((view) => ({ ...view, unreachable: failure.error }));
      remote.value = failure;
    });
    listWorkspacesWithOrder()
      .then(({ workspaces: data, order: stored }) => {
        if (!live) return;
        list.value = { status: 'loaded', data };
        order.value = stored;
        adoptWorkspaceDisplayName(data);
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
  const placeholder = <WorkspaceSwitcherSkeleton rows={skeleton.rows} manage={skeleton.manage} />;

  const open = (w: WorkspaceStatus, mode: WorkspaceOpenMode) => {
    contextId.value = null;
    onClose();
    void openWorkspaceIn(mode, w.id).catch((e: unknown) => {
      showToast(`Could not open ${w.name}: ${errorDetail(e)}`, 'error');
    });
  };

  return (
    <>
      {workspacesMenuRow({
        canList,
        manageHref,
        workspaceName: visibleWorkspaceName.value + (REMOTE_GATEWAY ? ` @${REMOTE_GATEWAY}` : ''),
        expanded: expanded.value,
        onToggle: () => {
          expanded.value = !expanded.value;
          rememberWorkspaceSwitcherExpanded(expanded.value);
        },
        onNavigate: onClose,
      })}
      <Disclosure open={expanded.value}>
        <LoadingFade
          showSkeleton={showSkeleton}
          skeleton={placeholder}
        >
          {loading ? <div class="brand-menu-ws-reserve" aria-hidden="true">{placeholder}</div> : workspaceSwitcherList({
            state: list.value,
            currentId: WORKSPACE_ID,
            manageHref,
            onActivate: (w, e) => {
              const mode = openModeForClick(e);
              if (mode !== null) open(w, mode);
            },
            onNavigate: onClose,
            contextId: contextId.value,
            onContext: (id) => { contextId.value = id; },
            onAlternate: open,
            remote: remote.value.status === 'loaded' ? remote.value.data : cachedRemote.current,
            remoteError: remote.value.status === 'failed' ? remote.value.error : null,
            order: order.value,
            onOpenRemote: (route, mode) => {
              onClose();
              void openWorkspaceIn(mode, route).catch((e: unknown) => {
                showToast(`Could not open ${route}: ${errorDetail(e)}`, 'error');
              });
            },
          })}
        </LoadingFade>
      </Disclosure>
    </>
  );
}
