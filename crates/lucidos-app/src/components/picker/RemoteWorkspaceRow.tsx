/**
 * The body of a picker row for a workspace on a remote gateway. It wears the
 * local row's shape: dot, name with a quiet `@gateway` tag, and a host line.
 *
 * A row opens the workspace through this gateway's relay, at `/<slug>@<name>/`.
 * Its ⋯ menu (`actions`) offers no Start, Stop, rename or delete: those belong
 * to the gateway that hosts the workspace. A gateway that did not answer keeps
 * its last rows, greyed and closed, and `UnreachableGatewayNotes` says why.
 */

import type { ComponentChildren } from 'preact';
import type { ListedWorkspace } from '../../api/client/control';
import {
  defaultOpenMode,
  middleClickActivates,
  middleClickHandler,
  openModeForClick,
  type WorkspaceOpenMode,
} from '../../utils/workspaceWindow';
import { workspaceFaultNote, workspaceState, workspaceStateLabel } from '../../utils/workspaceState';
import { remoteWorkspaceOpenable, remoteWorkspaceRoute, type RemoteGatewayView } from './remoteWorkspaces';

interface RemoteWorkspaceRowBodyProps {
  view: RemoteGatewayView;
  workspace: ListedWorkspace;
  onOpen: (route: string, mode: WorkspaceOpenMode) => void;
  onContextMenu?: (e: MouseEvent) => void;
  /** The row's ⋯ menu, drawn where a local row draws its actions. */
  actions: ComponentChildren;
}

export function RemoteWorkspaceRowBody({ view, workspace: w, onOpen, onContextMenu, actions }: RemoteWorkspaceRowBodyProps) {
  const route = remoteWorkspaceRoute(view, w);
  const state = workspaceState(w);
  const fault = workspaceFaultNote(w);
  // Nothing here can restart a remote engine, so an unhealthy row stays shut
  // rather than dropping the user into a page that cannot connect.
  const openable = remoteWorkspaceOpenable(view, w);
  const open = (mode: WorkspaceOpenMode | null) => {
    if (openable && mode !== null) onOpen(route, mode);
  };
  return (
    <div
      class="ws-picker-open"
      role={openable ? 'button' : undefined}
      tabIndex={openable ? 0 : undefined}
      aria-disabled={openable ? undefined : 'true'}
      aria-label={fault ? `${w.name} on ${view.gateway} · ${fault}` : `Open ${w.name} on ${view.gateway}`}
      onClick={(e) => open(openModeForClick(e))}
      onAuxClick={openable && middleClickActivates(state) ? middleClickHandler((e) => open(openModeForClick(e))) : undefined}
      onContextMenu={onContextMenu}
      onKeyDown={(e) => {
        if (e.target !== e.currentTarget) return;
        if (e.key === 'Enter' || e.key === ' ') {
          e.preventDefault();
          open(defaultOpenMode());
        }
      }}
    >
      <div class="ws-picker-line">
        <span
          class={`ws-picker-dot ws-picker-dot-${state}`}
          data-tooltip={workspaceStateLabel(w)}
          aria-hidden="true"
        />
        <div class="ws-picker-id">
          <span class="ws-picker-name">
            {w.name} <span class="ws-picker-gateway-tag">@{view.gateway}</span>
          </span>
          {view.host
            ? <span class="ws-picker-address ws-picker-host">On {view.host}</span>
            : <span class="ws-picker-address">{route}</span>}
        </div>
        <div class="ws-picker-actions" onClick={(e) => e.stopPropagation()} onAuxClick={(e) => e.stopPropagation()}>
          {actions}
        </div>
      </div>
      {fault && <p class="ws-picker-row-note">{fault}</p>}
    </div>
  );
}

/** One line per gateway that did not answer, naming it and saying why. */
export function UnreachableGatewayNotes({ views }: { views: readonly RemoteGatewayView[] }) {
  const silent = views.filter((view) => view.unreachable !== null);
  if (silent.length === 0) return null;
  return (
    <ul class="ws-picker-list ws-picker-remote-list" aria-label="Remote gateways not reachable">
      {silent.map((view) => (
        <li class="ws-picker-row ws-picker-row-unreachable" key={`gateway:${view.gateway}`}>
          <div class="ws-picker-open" aria-disabled="true">
            <div class="ws-picker-line">
              <span class="ws-picker-dot ws-picker-dot-unhealthy" aria-hidden="true" />
              <div class="ws-picker-id">
                <span class="ws-picker-name">{view.gateway}</span>
                <span class="ws-picker-address">Remote gateway, not reachable</span>
              </div>
            </div>
            <p class="ws-picker-row-note">{view.unreachable}</p>
          </div>
        </li>
      ))}
    </ul>
  );
}
