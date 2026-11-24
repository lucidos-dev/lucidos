
import type { WorkspaceOpenMode } from '../../utils/workspaceWindow';
import { openModeLabel } from '../../utils/workspaceWindow';
import { PopInIcon, PopOutIcon } from '../shared/icons';
import { Disclosure } from '../shared/Disclosure';

export function workspaceActionRow({ open, id, name, mode, indentClass, onActivate }: {
  open: boolean;
  id: string;
  name: string;
  mode: WorkspaceOpenMode | null;
  indentClass: string;
  onActivate: (mode: WorkspaceOpenMode) => void;
}) {
  return (
    <Disclosure key={`fold:${id}`} open={open && mode !== null}>
      {mode !== null && actionButton(id, name, mode, indentClass, onActivate)}
    </Disclosure>
  );
}

function actionButton(
  id: string, name: string, mode: WorkspaceOpenMode, indentClass: string,
  onActivate: (mode: WorkspaceOpenMode) => void,
) {
  const label = openModeLabel(mode);
  return (
    <button
      type="button"
      class={`brand-menu-ws-row brand-menu-ws-action ${indentClass}`}
      role="menuitem"
      aria-label={`${label}: ${name}`}
      onClick={() => onActivate(mode)}
      key={`window:${id}`}
    >
      {mode === 'separate' ? <PopOutIcon /> : <PopInIcon />}
      <span class="brand-menu-ws-name">{label}</span>
    </button>
  );
}
