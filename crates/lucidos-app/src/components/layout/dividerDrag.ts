/** The pointer half of every divider drag. It captures the pointer, so the drag
 *  survives the pointer leaving the handle for the content beside it. It holds
 *  the col-resize cursor and suppresses text selection page-wide, and marks the
 *  handle `data-dragging` for styling. Release or cancel undoes all of it and
 *  then calls `onEnd`.
 *
 *  Returns that same end function, so a caller that can unmount mid-drag can
 *  finish the drag itself rather than leave the body styles stuck. */
export function startDividerDrag(
  e: PointerEvent,
  onMove: (e: PointerEvent) => void,
  onEnd: () => void,
): () => void {
  const handle = e.currentTarget as HTMLElement;
  handle.setPointerCapture(e.pointerId);

  let ended = false;
  const end = () => {
    if (ended) return;
    ended = true;
    handle.removeEventListener('pointermove', onMove);
    handle.removeEventListener('pointerup', end);
    handle.removeEventListener('pointercancel', end);
    handle.removeAttribute('data-dragging');
    document.body.style.cursor = '';
    document.body.style.userSelect = '';
    onEnd();
  };

  handle.setAttribute('data-dragging', '');
  document.body.style.cursor = 'col-resize';
  document.body.style.userSelect = 'none';
  handle.addEventListener('pointermove', onMove);
  handle.addEventListener('pointerup', end);
  handle.addEventListener('pointercancel', end);
  return end;
}
