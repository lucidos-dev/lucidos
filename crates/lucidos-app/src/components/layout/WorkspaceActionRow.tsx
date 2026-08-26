/**
 * The ALTERNATE open mode, as a row of the Lucidos menu rather than a popover
 * over it. A right-click on a workspace row unfolds one of these under it.
 *
 * Two independent reasons it is not a nested `<Overlay>`. The panel is
 * `transform`ed and `overflow: hidden auto`, so a `position: fixed` child
 * resolves against it and is clipped by it. A portaled panel is also OUTSIDE
 * the menu for the dismiss contract. The first pointerdown on it would
 * therefore shut the menu and unmount the action with it.
 *
 * `WorkspaceRestartRow` renders its confirm inline for the second reason. The
 * switcher's own list unfolds this way too, so this is the shape the surface
 * has rather than a workaround.
 *
 * Its own module because BOTH menu groups that list workspaces unfold one:
 * `WorkspaceSwitcher.tsx` and `NotificationsMenuRows.tsx`. A copy each would
 * drift on the icon, the label, the accessible name or the key. Only the INDENT
 * differs, their rows leading with different columns, so that is what the
 * caller passes.
 */

import type { WorkspaceOpenMode } from '../../utils/workspaceWindow';
import { openModeLabel } from '../../utils/workspaceWindow';
import { PopInIcon, PopOutIcon } from '../shared/icons';

export function workspaceActionRow({ id, name, mode, indentClass, onActivate }: {
  /** The workspace this action belongs to. Its row sits directly above. */
  id: string;
  /** What to call that workspace out loud, since the label cannot name it. */
  name: string;
  mode: WorkspaceOpenMode;
  /** The host's own indent, putting this row's glyph on the name column of the
   *  row above it. See the two rules in styles/header-mark.css. */
  indentClass: string;
  onActivate: (mode: WorkspaceOpenMode) => void;
}) {
  const label = openModeLabel(mode);
  return (
    <button
      type="button"
      class={`brand-menu-ws-row brand-menu-ws-action ${indentClass}`}
      role="menuitem"
      // Names the workspace, which the visible label cannot: the row sits under
      // it, and an `aria-label` replaces the content a screen reader would read.
      aria-label={`${label}: ${name}`}
      onClick={() => onActivate(mode)}
      // A colon, because a slug cannot contain one and a workspace row's key is
      // a bare slug. `${id}-window` could not promise that: it collides with
      // the row of a workspace actually named `<id>-window`, and two siblings
      // sharing a key is how keyed diffing reuses the wrong node.
      key={`window:${id}`}
    >
      {mode === 'separate' ? <PopOutIcon /> : <PopInIcon />}
      <span class="brand-menu-ws-name">{label}</span>
    </button>
  );
}
