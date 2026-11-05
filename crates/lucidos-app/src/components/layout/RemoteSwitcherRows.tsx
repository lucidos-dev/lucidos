import type { ListedWorkspace } from '../../api/client/control';
import { CheckIcon } from '../shared/icons';
import { remoteWorkspaceOpenable, remoteWorkspaceRoute, type RemoteGatewayView } from '../../utils/remoteWorkspaces';
import { workspaceState, workspaceStateLabel } from '../../utils/workspaceState';

export function remoteSwitcherRows(views: readonly RemoteGatewayView[], currentId: string | null, onOpen: (route: string) => void) {
  return <div class="brand-menu-remote-workspaces">
    {views.map((view) => <section key={view.gateway} aria-label={`Remote gateway ${view.gateway}`}>
      <div class="brand-menu-ws-heading">@{view.gateway} · {view.host || view.gateway}</div>
      {view.unreachable && <p role="status">{view.unreachable}{view.lastSeenAt && ` · Last answer ${new Date(view.lastSeenAt).toLocaleString()}`}</p>}
      {view.workspaces.map((w: ListedWorkspace) => {
        const route = remoteWorkspaceRoute(view, w);
        const current = route === currentId;
        const openable = remoteWorkspaceOpenable(view, w) && !current;
        return <a key={route} href={openable ? `/${route}/` : undefined}
          role="menuitem" class={`brand-menu-ws-row${current ? ' is-current' : ''}`} aria-current={current ? 'page' : undefined}
          aria-disabled={openable ? undefined : 'true'}
          onClick={(e) => { if (!openable) { e.preventDefault(); return; } if (!e.metaKey && !e.ctrlKey && !e.shiftKey && !e.altKey) { e.preventDefault(); onOpen(route); } }}>
          <span class={`ws-picker-dot ws-picker-dot-${workspaceState(w)}`} aria-hidden="true" />
          <span class="brand-menu-ws-name">{w.name} <span class="remote-workspace-gateway">@{view.gateway}</span> <small>remote</small>
            <small>{view.host || view.gateway} · {remoteWorkspaceOpenable(view, w) ? workspaceStateLabel(w) : view.unreachable || 'Not running. Start it on the remote gateway.'}</small>
          </span>
          {current && <CheckIcon className="brand-menu-ws-check" />}
        </a>;
      })}
    </section>)}
  </div>;
}
