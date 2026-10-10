/** The pointer half of every divider drag. It captures the pointer, so the drag
 *  survives the pointer leaving the handle for the content beside it. It holds
 *  the col-resize cursor and suppresses text selection page-wide, and marks the
 *  handle `data-dragging` for styling. Release or cancel undoes all of it and
 *  then calls `onEnd`.
 *
 *  Moves reach `onMove` only once the pointer has left the press point. Chromium sends
 *  one while the pointer is captured, and a clamp on it would resize the pane
 *  under the first click of a double-click.
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
  const downX = e.clientX;
  const downY = e.clientY;
  let travelled = false;
  const onTravel = (m: PointerEvent) => {
    travelled ||= m.clientX !== downX || m.clientY !== downY;
    if (travelled) onMove(m);
  };

  let ended = false;
  const end = () => {
    if (ended) return;
    ended = true;
    handle.removeEventListener('pointermove', onTravel);
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
  handle.addEventListener('pointermove', onTravel);
  handle.addEventListener('pointerup', end);
  handle.addEventListener('pointercancel', end);
  return end;
}
