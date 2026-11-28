
import { isTauri } from './platform';
import { opensContextMenu, selectionCovers, shouldSuppressContextMenu, textAt } from '@lucidos/context-menu';

let installed = false;

export function installNativeContextMenuPolicy(): void {
  if (installed) return;
  installed = true;
  if (!isTauri()) return;

  let menuPress = false;
  let selectionAtPress = false;
  document.addEventListener('pointerdown', (e) => {
    menuPress = opensContextMenu(e);
    selectionAtPress = menuPress
      && selectionCovers(window.getSelection(), { x: e.clientX, y: e.clientY });
  }, { capture: true });

  document.addEventListener('contextmenu', (e) => {
    const point = { x: e.clientX, y: e.clientY };
    const fromPointer = menuPress;
    const heldSelection = selectionAtPress;
    menuPress = selectionAtPress = false;
    const textUnderPointer = () => !fromPointer || textAt(e.target, point);
    if (!shouldSuppressContextMenu(e, heldSelection, textUnderPointer)) return;
    e.preventDefault();
    const selection = window.getSelection();
    if (selectionCovers(selection, point)) selection?.removeAllRanges();
  });
}
