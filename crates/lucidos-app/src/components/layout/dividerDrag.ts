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
