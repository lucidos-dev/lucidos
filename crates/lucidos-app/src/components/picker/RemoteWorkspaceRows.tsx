/**
 * The picker's rows for workspaces on remote gateways, under the local list.
 *
 * A row opens the workspace through this gateway's relay, at `/<slug>@<name>/`.
 * It offers no Start, Stop, rename or delete: those belong to the gateway that
 * hosts the workspace. A gateway that did not answer keeps its last rows,
 * greyed and closed, under a line saying why.
 */

import type { ListedWorkspace } from '../../api/client/control';
import {
  defaultOpenMode,
  middleClickHandler,
  openModeForClick,
  type WorkspaceOpenMode,
} from '../../utils/workspaceWindow';
import { workspaceFaultNote, workspaceState, workspaceStateLabel } from '../../utils/workspaceState';
import { remoteWorkspaceRoute, type RemoteGatewayView } from './remoteWorkspaces';

interface RemoteWorkspaceRowsProps {
  views: readonly RemoteGatewayView[];
  onOpen: (route: string, mode: WorkspaceOpenMode) => void;
}

export function RemoteWorkspaceRows({ views, onOpen }: RemoteWorkspaceRowsProps) {
  if (views.length === 0) return null;
  return (
    <ul class="ws-picker-list ws-picker-remote-list" aria-label="Workspaces on remote gateways">
      {views.flatMap((view) => [
        ...(view.unreachable !== null ? [unreachableRow(view, view.unreachable)] : []),
        ...view.workspaces.map((w) => remoteRow(view, w, onOpen)),
      ])}
    </ul>
  );
}

/** The line naming a gateway that did not answer, and why. */
function unreachableRow(view: RemoteGatewayView, reason: string) {
  return (
    <li class="ws-picker-row ws-picker-row-unreachable" key={`gateway:${view.gateway}`}>
      <div class="ws-picker-open" aria-disabled="true">
        <div class="ws-picker-line">
          <span class="ws-picker-dot ws-picker-dot-unhealthy" aria-hidden="true" />
          <div class="ws-picker-id">
            <span class="ws-picker-name">{view.gateway}</span>
            <span class="ws-picker-address">Remote gateway, not reachable</span>
          </div>
        </div>
        <p class="ws-picker-row-note">{reason}</p>
      </div>
    </li>
  );
}

function remoteRow(
  view: RemoteGatewayView,
  w: ListedWorkspace,
  onOpen: (route: string, mode: WorkspaceOpenMode) => void,
) {
  const route = remoteWorkspaceRoute(view, w);
  const state = workspaceState(w);
  const fault = workspaceFaultNote(w);
  // Nothing here can restart a remote engine, so an unhealthy row stays shut
  // rather than dropping the user into a page that cannot connect.
  const openable = view.unreachable === null && state !== 'unhealthy';
  const open = (mode: WorkspaceOpenMode | null) => {
    if (openable && mode !== null) onOpen(route, mode);
  };
  return (
    <li
      class={`ws-picker-row${view.unreachable !== null ? ' ws-picker-row-unreachable' : ''}`}
      key={route}
    >
      <div
        class="ws-picker-open"
        role={openable ? 'button' : undefined}
        tabIndex={openable ? 0 : undefined}
        aria-disabled={openable ? undefined : 'true'}
        aria-label={fault ? `${w.name} on ${view.gateway} · ${fault}` : `Open ${w.name} on ${view.gateway}`}
        onClick={(e) => open(openModeForClick(e))}
        onAuxClick={openable ? middleClickHandler((e) => open(openModeForClick(e))) : undefined}
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
            <span class="ws-picker-name">{w.name}</span>
            <span class="ws-picker-address">{route}</span>
          </div>
        </div>
        {fault && <p class="ws-picker-row-note">{fault}</p>}
      </div>
    </li>
  );
}
