import { connectionStatus, workspaceName } from '../../store/store';

export function ConnectionStatus() {
  const status = connectionStatus.value;
  const name = workspaceName.value;

  return (
    <>
      <span class="connection-status-inline">
        <span class={`status-dot ${status}`} />
      </span>
      {name && <span class="workspace-name-label">{name}</span>}
    </>
  );
}
