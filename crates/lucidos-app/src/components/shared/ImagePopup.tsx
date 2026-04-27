import { useRef, useEffect } from 'preact/hooks';
import { popupImageSrc } from '../../store/store';
import { CloseIcon } from './icons';
import { ModalOverlay } from './ModalOverlay';

const MIN_SCALE = 1;
const MAX_SCALE = 10;
const WHEEL_FACTOR = 0.002;

export function ImagePopup() {
  const src = popupImageSrc.value;
  const imgRef = useRef<HTMLImageElement>(null);
  const stateRef = useRef({ scale: 1, tx: 0, ty: 0 });
  const dragRef = useRef({ active: false, startX: 0, startY: 0, originTx: 0, originTy: 0 });
  const pinchRef = useRef<{ dist: number; scale: number } | null>(null);

  // Native listener with { passive: false } — Preact's onWheel can't preventDefault on passive listeners
  useEffect(() => {
    const img = imgRef.current;
    if (!img) return;
    stateRef.current = { scale: 1, tx: 0, ty: 0 };
    img.style.transform = '';
    img.style.cursor = 'zoom-in';
    function handleWheel(e: WheelEvent) {
      e.preventDefault();
      const s = stateRef.current;
      const delta = -e.deltaY * WHEEL_FACTOR * s.scale;
      zoomAt(e.clientX, e.clientY, s.scale + delta);
    }
    img.addEventListener('wheel', handleWheel, { passive: false });
    return () => img.removeEventListener('wheel', handleWheel);
  }, [src]);

  if (!src) return null;

  function applyTransform() {
    const img = imgRef.current;
    if (!img) return;
    const { scale, tx, ty } = stateRef.current;
    img.style.transform = `translate(${tx}px, ${ty}px) scale(${scale})`;
    img.style.cursor = scale > 1 ? 'grab' : 'zoom-in';
  }

  function clampPan() {
    const img = imgRef.current;
    if (!img) return;
    const s = stateRef.current;
    if (s.scale <= 1) { s.tx = 0; s.ty = 0; return; }
    const container = img.parentElement!.getBoundingClientRect();
    const overflowX = Math.max(0, (img.offsetWidth * s.scale - container.width) / 2);
    const overflowY = Math.max(0, (img.offsetHeight * s.scale - container.height) / 2);
    s.tx = Math.max(-overflowX, Math.min(overflowX, s.tx));
    s.ty = Math.max(-overflowY, Math.min(overflowY, s.ty));
  }

  function zoomAt(clientX: number, clientY: number, newScale: number) {
    const img = imgRef.current;
    if (!img) return;
    const s = stateRef.current;
    const clamped = Math.max(MIN_SCALE, Math.min(MAX_SCALE, newScale));
    const ratio = clamped / s.scale;
    // Natural center of the image in viewport coords (unaffected by transform)
    const container = img.parentElement!.getBoundingClientRect();
    const ncx = container.left + img.offsetLeft + img.offsetWidth / 2;
    const ncy = container.top + img.offsetTop + img.offsetHeight / 2;
    // Adjust translation so the point under the cursor stays fixed
    s.tx = s.tx + (1 - ratio) * (clientX - ncx - s.tx);
    s.ty = s.ty + (1 - ratio) * (clientY - ncy - s.ty);
    s.scale = clamped;
    clampPan();
    applyTransform();
  }

  function onPointerDown(e: PointerEvent) {
    if (e.button !== 0) return;
    const s = stateRef.current;
    if (s.scale <= 1) return;
    e.preventDefault();
    dragRef.current = { active: true, startX: e.clientX, startY: e.clientY, originTx: s.tx, originTy: s.ty };
    const img = imgRef.current;
    if (img) {
      img.setPointerCapture(e.pointerId);
      img.style.cursor = 'grabbing';
    }
  }

  function onPointerMove(e: PointerEvent) {
    const d = dragRef.current;
    if (!d.active) return;
    e.preventDefault();
    const s = stateRef.current;
    s.tx = d.originTx + (e.clientX - d.startX);
    s.ty = d.originTy + (e.clientY - d.startY);
    clampPan();
    applyTransform();
  }

  function onPointerUp(e: PointerEvent) {
    dragRef.current.active = false;
    const img = imgRef.current;
    if (img) {
      img.releasePointerCapture(e.pointerId);
      img.style.cursor = stateRef.current.scale > 1 ? 'grab' : 'zoom-in';
    }
  }

  function onTouchStart(e: TouchEvent) {
    if (e.touches.length === 2) {
      e.preventDefault();
      const dx = e.touches[0].clientX - e.touches[1].clientX;
      const dy = e.touches[0].clientY - e.touches[1].clientY;
      pinchRef.current = { dist: Math.hypot(dx, dy), scale: stateRef.current.scale };
    }
  }

  function onTouchMove(e: TouchEvent) {
    if (e.touches.length === 2 && pinchRef.current) {
      e.preventDefault();
      const dx = e.touches[0].clientX - e.touches[1].clientX;
      const dy = e.touches[0].clientY - e.touches[1].clientY;
      const dist = Math.hypot(dx, dy);
      const cx = (e.touches[0].clientX + e.touches[1].clientX) / 2;
      const cy = (e.touches[0].clientY + e.touches[1].clientY) / 2;
      const newScale = pinchRef.current.scale * (dist / pinchRef.current.dist);
      zoomAt(cx, cy, newScale);
    }
  }

  function onTouchEnd() {
    pinchRef.current = null;
  }

  function onDoubleClick(e: MouseEvent) {
    e.preventDefault();
    e.stopPropagation();
    const s = stateRef.current;
    if (s.scale > 1) {
      s.scale = 1; s.tx = 0; s.ty = 0;
      applyTransform();
    } else {
      zoomAt(e.clientX, e.clientY, 3);
    }
  }

  function close() {
    popupImageSrc.value = null;
  }

  return (
    <ModalOverlay onClose={stateRef.current.scale > 1 ? undefined : close} class="image-popup">
      <div class="image-popup-content">
        <button class="image-popup-close icon-btn" onClick={close} aria-label="Close" data-tooltip="Close">
          <CloseIcon />
        </button>
        <button class="floating-mobile-close" onClick={close} aria-label="Close">
          <CloseIcon />
        </button>
        <img
          ref={imgRef}
          src={src}
          alt="Full size"
          draggable={false}
          onPointerDown={onPointerDown}
          onPointerMove={onPointerMove}
          onPointerUp={onPointerUp}
          onPointerCancel={onPointerUp}
          onTouchStart={onTouchStart}
          onTouchMove={onTouchMove}
          onTouchEnd={onTouchEnd}
          onDblClick={onDoubleClick}
          style="cursor: zoom-in;"
        />
      </div>
    </ModalOverlay>
  );
}
