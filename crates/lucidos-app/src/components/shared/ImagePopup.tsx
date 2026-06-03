import { useRef, useEffect, useLayoutEffect, useState } from 'preact/hooks';
import { popupImage } from '../../store/store';
import { CloseIcon, ChevronLeftIcon, ChevronRightIcon } from './icons';
import { ModalOverlay } from './ModalOverlay';
import {
  fingerDistance,
  fingerMidpoint,
  computePinchUpdate,
  clampPanTransform,
  computeZoomAt,
  type PinchInitial,
} from '../../utils/pinchGesture';
import { SwipeTouch } from '../../utils/swipe';

const MIN_SCALE = 1;
const MAX_SCALE = 10;
const WHEEL_FACTOR = 0.002;
const DOUBLE_TAP_SCALE = 3;
const SWIPE_COMMIT_MS = 220;
const SWIPE_SNAP_BACK_MS = 200;
// transitionend isn't 100% reliable (background tab, dropped paint). Safety
// timer guarantees the post-animation cleanup runs.
const TRANSITION_TIMEOUT_MS = 400;

// Strip rest position: slides are positioned by shortestDelta around the
// current index, so the centered slide always sits at left=0 and the strip
// translates to 0 to show it.
const STRIP_REST_TRANSFORM = 'translate3d(0, 0, 0)';

// Shortest signed distance from c to i in a ring of size n. Returns a value
// in (-n/2, n/2]. Used to position each slide adjacent to the current one so
// last↔first wraps slide in from the correct side instead of the strip
// scrolling backward through every intermediate image.
function shortestDelta(i: number, c: number, n: number): number {
  if (n <= 1) return 0;
  let d = ((i - c) % n + n) % n;
  if (d > n / 2) d -= n;
  return d;
}

function step(delta: -1 | 1) {
  const s = popupImage.value;
  if (!s || s.images.length <= 1) return;
  const next = (s.index + delta + s.images.length) % s.images.length;
  popupImage.value = { images: s.images, index: next };
}

interface LayoutCache {
  containerW: number;
  containerH: number;
  imgW: number;
  imgH: number;
  natCenterX: number;
  natCenterY: number;
}

function parseTranslateX(el: HTMLElement): number {
  const t = getComputedStyle(el).transform;
  if (!t || t === 'none') return 0;
  const m = t.match(/matrix(?:3d)?\(([^)]+)\)/);
  if (!m) return 0;
  const v = m[1].split(',').map(s => parseFloat(s.trim()));
  if (v.length === 6) return v[4];
  if (v.length === 16) return v[12];
  return 0;
}

export function ImagePopup() {
  const state = popupImage.value;
  const stripRef = useRef<HTMLDivElement>(null);
  const zoomRef = useRef({ scale: 1, tx: 0, ty: 0 });
  const zoomedImgRef = useRef<HTMLImageElement | null>(null);
  const dragRef = useRef({ active: false, startX: 0, startY: 0, originTx: 0, originTy: 0 });
  const pinchRef = useRef<PinchInitial | null>(null);
  const swipeRef = useRef<SwipeTouch | null>(null);
  if (swipeRef.current === null) swipeRef.current = new SwipeTouch();
  const swipe = swipeRef.current;
  const swipeDxRef = useRef<number | null>(null);
  // Captured at touchstart: the strip's visual offset at that moment (could be
  // mid-animation), and width. Slide positions are computed from the live
  // signal index, so we don't need to remember the start index.
  const dragStartRef = useRef<{ offset: number; w: number } | null>(null);
  const layoutRef = useRef<LayoutCache | null>(null);
  const rafRef = useRef<number | null>(null);
  // Cleanup for the in-flight commit/snap-back transitionend listener.
  const transitionCleanupRef = useRef<(() => void) | null>(null);
  // The index a pending commit is animating toward, plus the index it started
  // from. We need both to compensate the strip transform when a new gesture
  // flushes the pending commit mid-animation.
  const pendingCommitRef = useRef<{ from: number; to: number } | null>(null);
  // True while a touch gesture is in progress; tells the layout effect not to
  // clobber the imperatively-managed transform.
  const touchActiveRef = useRef(false);
  // Tracks the strip DOM node we last initialised — first set on each mount
  // skips the transition so opening the popup doesn't slide.
  const lastStripRef = useRef<HTMLDivElement | null>(null);
  const [chromeHidden, setChromeHidden] = useState(false);
  // Reset on each open so the chrome is always visible when re-launching.
  useEffect(() => { if (state) setChromeHidden(false); }, [state !== null]);

  const total = state?.images.length ?? 0;
  const hasNav = total > 1;

  // Strip rests at translateX(0); slides are positioned by shortest delta from
  // the current index, so the centered slide is always at left=0. After a
  // swipe commit / keyboard step, the imperative animation lands at ±W and
  // this effect snaps the strip back to 0 with no transition — slides have
  // repositioned in the same render tick, so the visual stays put.
  useLayoutEffect(() => {
    const strip = stripRef.current;
    if (!strip || !state || touchActiveRef.current) return;
    strip.style.transition = 'none';
    strip.style.transform = STRIP_REST_TRANSFORM;
    if (lastStripRef.current !== strip) {
      void strip.offsetWidth;
      lastStripRef.current = strip;
    }
  }, [state?.index]);

  // Reset zoom when navigating to a new image.
  useLayoutEffect(() => {
    zoomRef.current = { scale: 1, tx: 0, ty: 0 };
    layoutRef.current = null;
    const old = zoomedImgRef.current;
    if (old) {
      old.style.transform = '';
      old.style.cursor = 'zoom-in';
      zoomedImgRef.current = null;
    }
  }, [state?.index]);

  // Gesture handlers attached once to the strip. They read live state via the
  // popupImage signal so they don't need to re-attach on navigation.
  useEffect(() => {
    if (!stripRef.current) return;
    // TS doesn't narrow refs through nested function declarations; capture as a
    // non-null local so every closure inherits the narrowed type.
    const strip: HTMLDivElement = stripRef.current;

    function getCurrentImg(): HTMLImageElement | null {
      const s = popupImage.value;
      if (!s) return null;
      const slide = strip.children[s.index] as HTMLDivElement | undefined;
      return slide?.querySelector<HTMLImageElement>('img') ?? null;
    }

    function captureLayout(): LayoutCache | null {
      const img = getCurrentImg();
      if (!img) return null;
      const slide = img.parentElement;
      if (!slide) return null;
      const slideRect = slide.getBoundingClientRect();
      const imgRect = img.getBoundingClientRect();
      return {
        containerW: slideRect.width,
        containerH: slideRect.height,
        imgW: imgRect.width,
        imgH: imgRect.height,
        natCenterX: imgRect.left + imgRect.width / 2,
        natCenterY: imgRect.top + imgRect.height / 2,
      };
    }
    function ensureLayout(): LayoutCache | null {
      if (!layoutRef.current) layoutRef.current = captureLayout();
      return layoutRef.current;
    }

    function applyZoom() {
      const img = getCurrentImg();
      if (!img) return;
      const { scale, tx, ty } = zoomRef.current;
      img.style.transform = `translate3d(${tx}px, ${ty}px, 0) scale(${scale})`;
      img.style.cursor = scale > 1 ? 'grab' : 'zoom-in';
      zoomedImgRef.current = scale > 1 ? img : null;
    }
    function scheduleApplyZoom() {
      if (rafRef.current !== null) return;
      rafRef.current = requestAnimationFrame(() => {
        rafRef.current = null;
        applyZoom();
      });
    }
    function clamp() {
      const layout = ensureLayout();
      if (!layout) return;
      const z = zoomRef.current;
      const c = clampPanTransform(
        z.scale, z.tx, z.ty,
        layout.containerW, layout.containerH,
        layout.imgW, layout.imgH,
      );
      z.tx = c.tx; z.ty = c.ty;
    }
    function zoomAt(clientX: number, clientY: number, newScale: number) {
      const layout = ensureLayout();
      if (!layout) return;
      const z = zoomRef.current;
      const u = computeZoomAt(
        z.scale, z.tx, z.ty,
        clientX, clientY, newScale,
        layout.natCenterX, layout.natCenterY,
        MIN_SCALE, MAX_SCALE,
      );
      z.scale = u.scale; z.tx = u.tx; z.ty = u.ty;
      clamp();
      scheduleApplyZoom();
    }
    function setPan(tx: number, ty: number) {
      const z = zoomRef.current;
      z.tx = tx; z.ty = ty;
      clamp();
      scheduleApplyZoom();
    }
    function resetZoom() {
      zoomRef.current = { scale: 1, tx: 0, ty: 0 };
      scheduleApplyZoom();
    }

    function cancelSwipeRaf() {
      if (rafRef.current !== null) {
        cancelAnimationFrame(rafRef.current);
        rafRef.current = null;
      }
      swipeDxRef.current = null;
    }

    function cancelTransition() {
      const cleanup = transitionCleanupRef.current;
      transitionCleanupRef.current = null;
      if (cleanup) cleanup();
    }

    // If a previous swipe started a commit animation that hasn't landed yet,
    // advance the signal now AND compensate the strip transform so the visual
    // stays put. After the signal update, slides re-render based on the new
    // index — we shift the strip by `delta * W` to keep every visible slide in
    // the same on-screen position.
    function flushPendingCommit() {
      const pending = pendingCommitRef.current;
      pendingCommitRef.current = null;
      if (!pending) return;
      const cur = popupImage.value;
      if (!cur || cur.index === pending.to) return;
      const w = strip.offsetWidth;
      const tBefore = parseTranslateX(strip);
      const len = cur.images.length;
      const delta = shortestDelta(pending.to, pending.from, len);
      popupImage.value = { images: cur.images, index: pending.to };
      // After the signal write, slides have repositioned. Compensate so
      // visuals match: visual = newRelLeft*W + tNew, and we want it to equal
      // oldRelLeft*W + tBefore. For the new center (newRelLeft=0,
      // oldRelLeft=delta), tNew = delta*W + tBefore.
      strip.style.transition = 'none';
      strip.style.transform = `translate3d(${delta * w + tBefore}px, 0, 0)`;
    }

    // Run `done` exactly once when the strip's transform transition ends, or
    // after TRANSITION_TIMEOUT_MS as a safety net. Cancel previous listener.
    function onTransitionDone(done: () => void) {
      cancelTransition();
      let fired = false;
      const finish = () => {
        if (fired) return;
        fired = true;
        strip.removeEventListener('transitionend', onEnd);
        clearTimeout(timeoutId);
        if (transitionCleanupRef.current === cleanup) {
          transitionCleanupRef.current = null;
        }
        done();
      };
      const onEnd = (e: TransitionEvent) => {
        if (e.propertyName === 'transform' && e.target === strip) finish();
      };
      const cleanup = () => {
        fired = true;
        strip.removeEventListener('transitionend', onEnd);
        clearTimeout(timeoutId);
      };
      strip.addEventListener('transitionend', onEnd);
      const timeoutId = window.setTimeout(finish, TRANSITION_TIMEOUT_MS);
      transitionCleanupRef.current = cleanup;
    }

    function scheduleSwipeWrite() {
      if (rafRef.current !== null) return;
      rafRef.current = requestAnimationFrame(() => {
        rafRef.current = null;
        const dx = swipeDxRef.current;
        const drag = dragStartRef.current;
        if (!strip || dx === null || !drag) return;
        strip.style.transform = `translate3d(${drag.offset + dx}px, 0, 0)`;
      });
    }

    function handleWheel(e: WheelEvent) {
      e.preventDefault();
      const z = zoomRef.current;
      const delta = -e.deltaY * WHEEL_FACTOR * z.scale;
      zoomAt(e.clientX, e.clientY, z.scale + delta);
    }

    function handleTouchStart(e: TouchEvent) {
      if (e.touches.length === 2) {
        e.preventDefault();
        // Pinch supersedes any in-flight swipe and freezes the strip where it
        // is — captureLayout reads the centered image's bounds against the
        // current visual position, not the rest position.
        swipe.cancel();
        cancelSwipeRaf();
        cancelTransition();
        // Lock BEFORE flushing so the layout effect can't reset transform
        // between flush's signal-write and the captureLayout below.
        touchActiveRef.current = true;
        flushPendingCommit();
        dragRef.current.active = false;
        dragStartRef.current = null;
        const cur = parseTranslateX(strip);
        strip.style.transition = 'none';
        strip.style.transform = `translate3d(${cur}px, 0, 0)`;
        const layout = captureLayout();
        if (!layout) return;
        layoutRef.current = layout;
        const t0 = e.touches[0];
        const t1 = e.touches[1];
        const mid = fingerMidpoint(t0.clientX, t0.clientY, t1.clientX, t1.clientY);
        const dist = fingerDistance(t0.clientX, t0.clientY, t1.clientX, t1.clientY);
        const z = zoomRef.current;
        pinchRef.current = {
          scale: z.scale, tx: z.tx, ty: z.ty,
          midX: mid.x, midY: mid.y, dist,
          natCenterX: layout.natCenterX, natCenterY: layout.natCenterY,
        };
        return;
      }
      if (e.touches.length !== 1) return;
      if (zoomRef.current.scale > 1) return;  // pointer-pan handles zoomed drag
      cancelSwipeRaf();
      cancelTransition();
      // Lock BEFORE flushing — flushPendingCommit writes the signal, and we
      // don't want the layout effect to race ahead with a transform reset
      // before we've captured the visual position.
      touchActiveRef.current = true;
      flushPendingCommit();
      const s = popupImage.value;
      if (!s || s.images.length <= 1) { releaseGesture(); return; }
      const w = strip.offsetWidth;
      const offset = parseTranslateX(strip);
      strip.style.transition = 'none';
      strip.style.transform = `translate3d(${offset}px, 0, 0)`;
      dragStartRef.current = { offset, w };
      const t = e.touches[0];
      swipe.start(t.clientX, t.clientY);
    }

    function handleTouchMove(e: TouchEvent) {
      if (e.touches.length === 2 && pinchRef.current) {
        e.preventDefault();
        const t0 = e.touches[0];
        const t1 = e.touches[1];
        const mid = fingerMidpoint(t0.clientX, t0.clientY, t1.clientX, t1.clientY);
        const dist = fingerDistance(t0.clientX, t0.clientY, t1.clientX, t1.clientY);
        const u = computePinchUpdate(pinchRef.current, mid.x, mid.y, dist, MIN_SCALE, MAX_SCALE);
        const z = zoomRef.current;
        z.scale = u.scale; z.tx = u.tx; z.ty = u.ty;
        clamp();
        scheduleApplyZoom();
        return;
      }
      if (e.touches.length === 1) {
        const t = e.touches[0];
        const dx = swipe.move(t.clientX, t.clientY);
        if (dx === null) return;
        e.preventDefault();
        swipeDxRef.current = dx;
        scheduleSwipeWrite();
      }
    }

    function releaseGesture() {
      touchActiveRef.current = false;
      dragStartRef.current = null;
      pendingCommitRef.current = null;
    }

    function handleTouchEnd(e: TouchEvent) {
      const wasHorizontal = swipe.isHorizontal;
      const result = swipe.end(window.innerWidth || 1);
      const drag = dragStartRef.current;

      if (e.touches.length === 0) layoutRef.current = null;
      pinchRef.current = null;

      // Pinch sets touchActiveRef without dragStartRef; drop the gesture lock
      // when all fingers lift, otherwise the layout effect bails forever.
      if (!drag) {
        if (e.touches.length === 0) releaseGesture();
        return;
      }

      if (result !== 0) {
        cancelSwipeRaf();
        // Length read live — useEffect captures `total` from the initial
        // render and won't see images-array changes.
        const cur0 = popupImage.value;
        const len = cur0?.images.length ?? 0;
        if (len <= 1) { releaseGesture(); return; }
        const fromIndex = cur0!.index;
        const newIndex = (fromIndex + result + len) % len;
        // Animate exactly one slot in the swipe direction; the wrapped slide
        // already sits at ±W (see shortestDelta) and rides in from that side.
        const targetPx = -result * drag.w;
        strip.style.transition = `transform ${SWIPE_COMMIT_MS}ms ease-out`;
        strip.style.transform = `translate3d(${targetPx}px, 0, 0)`;
        pendingCommitRef.current = { from: fromIndex, to: newIndex };
        onTransitionDone(() => {
          // Snap strip back to 0 BEFORE the signal update — slides reposition
          // in the same render tick, so the centered slide stays on screen.
          // Without the snap the layout effect would re-set transform to 0
          // with the default transition, animating a backward slide.
          strip.style.transition = 'none';
          strip.style.transform = STRIP_REST_TRANSFORM;
          void strip.offsetWidth;
          const cur = popupImage.value;
          releaseGesture();
          if (cur && cur.index !== newIndex) {
            popupImage.value = { images: cur.images, index: newIndex };
          }
        });
        return;
      }

      if (wasHorizontal) {
        cancelSwipeRaf();
        strip.style.transition = `transform ${SWIPE_SNAP_BACK_MS}ms ease-out`;
        strip.style.transform = STRIP_REST_TRANSFORM;
        onTransitionDone(releaseGesture);
        return;
      }

      releaseGesture();
    }

    function onPointerDown(e: PointerEvent) {
      if (e.button !== 0) return;
      if (pinchRef.current) return;
      const z = zoomRef.current;
      if (z.scale <= 1) return;
      e.preventDefault();
      dragRef.current = {
        active: true,
        startX: e.clientX,
        startY: e.clientY,
        originTx: z.tx,
        originTy: z.ty,
      };
      const img = getCurrentImg();
      if (img) {
        try { img.setPointerCapture(e.pointerId); } catch { /* not capturable */ }
        img.style.cursor = 'grabbing';
      }
    }
    function onPointerMove(e: PointerEvent) {
      const d = dragRef.current;
      if (!d.active || pinchRef.current) return;
      e.preventDefault();
      setPan(d.originTx + (e.clientX - d.startX), d.originTy + (e.clientY - d.startY));
    }
    function onPointerUp(e: PointerEvent) {
      dragRef.current.active = false;
      const img = getCurrentImg();
      if (img) {
        try { img.releasePointerCapture(e.pointerId); } catch { /* not captured */ }
        img.style.cursor = zoomRef.current.scale > 1 ? 'grab' : 'zoom-in';
      }
    }
    function onDoubleClick(e: MouseEvent) {
      e.preventDefault();
      e.stopPropagation();
      if (zoomRef.current.scale > 1) {
        resetZoom();
      } else {
        zoomAt(e.clientX, e.clientY, DOUBLE_TAP_SCALE);
      }
    }

    function onClick(e: MouseEvent) {
      // detail>1 is the second click of a dblclick; skip so we don't
      // toggle twice on the way to dblclick-zoom.
      if (e.detail > 1) return;
      if (zoomRef.current.scale > 1) return;
      // Backdrop clicks close — the modal-overlay handles the outer
      // padding, the strip handles the dark area inside content.
      if (e.target instanceof HTMLImageElement) {
        setChromeHidden(v => !v);
      } else {
        popupImage.value = null;
      }
    }

    strip.addEventListener('wheel', handleWheel, { passive: false });
    strip.addEventListener('touchstart', handleTouchStart, { passive: false });
    strip.addEventListener('touchmove', handleTouchMove, { passive: false });
    strip.addEventListener('touchend', handleTouchEnd, { passive: false });
    strip.addEventListener('touchcancel', handleTouchEnd, { passive: false });
    strip.addEventListener('pointerdown', onPointerDown);
    strip.addEventListener('pointermove', onPointerMove);
    strip.addEventListener('pointerup', onPointerUp);
    strip.addEventListener('pointercancel', onPointerUp);
    strip.addEventListener('dblclick', onDoubleClick);
    strip.addEventListener('click', onClick);
    return () => {
      strip.removeEventListener('wheel', handleWheel);
      strip.removeEventListener('touchstart', handleTouchStart);
      strip.removeEventListener('touchmove', handleTouchMove);
      strip.removeEventListener('touchend', handleTouchEnd);
      strip.removeEventListener('touchcancel', handleTouchEnd);
      strip.removeEventListener('pointerdown', onPointerDown);
      strip.removeEventListener('pointermove', onPointerMove);
      strip.removeEventListener('pointerup', onPointerUp);
      strip.removeEventListener('pointercancel', onPointerUp);
      strip.removeEventListener('dblclick', onDoubleClick);
      strip.removeEventListener('click', onClick);
      cancelSwipeRaf();
      cancelTransition();
      pinchRef.current = null;
      dragStartRef.current = null;
      layoutRef.current = null;
    };
    // ImagePopup is always mounted at App root; the strip DOM only exists
    // when state is non-null. Re-run on open/close transitions so listeners
    // attach to the freshly-mounted strip.
  }, [state !== null]);

  useEffect(() => {
    if (!hasNav) return;
    function onKey(e: KeyboardEvent) {
      if (e.key === 'ArrowLeft') { e.preventDefault(); step(-1); }
      else if (e.key === 'ArrowRight') { e.preventDefault(); step(1); }
    }
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [hasNav]);

  // Recompute strip transform on viewport resize / orientation change.
  // Slides reposition automatically via render; we just need to stay at rest.
  useEffect(() => {
    if (!state) return;
    function onResize() {
      const strip = stripRef.current;
      if (!strip || touchActiveRef.current) return;
      strip.style.transition = 'none';
      strip.style.transform = STRIP_REST_TRANSFORM;
    }
    window.addEventListener('resize', onResize);
    window.addEventListener('orientationchange', onResize);
    return () => {
      window.removeEventListener('resize', onResize);
      window.removeEventListener('orientationchange', onResize);
    };
  }, [state !== null]);

  if (!state) return null;

  function close() {
    popupImage.value = null;
  }

  return (
    <ModalOverlay onClose={zoomRef.current.scale > 1 ? undefined : close} class="image-popup">
      <div class={`image-popup-content${chromeHidden ? ' chrome-hidden' : ''}`}>
        <button class="image-popup-close icon-btn" onClick={close} aria-label="Close" data-tooltip="Close">
          <CloseIcon />
        </button>
        <button class="floating-mobile-close" onClick={close} aria-label="Close">
          <CloseIcon />
        </button>
        {hasNav && (
          <>
            <button
              class="image-popup-nav image-popup-nav-prev"
              onClick={(e) => { e.stopPropagation(); step(-1); }}
              aria-label="Previous image"
              data-tooltip="Previous"
            >
              <ChevronLeftIcon />
            </button>
            <button
              class="image-popup-nav image-popup-nav-next"
              onClick={(e) => { e.stopPropagation(); step(1); }}
              aria-label="Next image"
              data-tooltip="Next"
            >
              <ChevronRightIcon />
            </button>
            <div class="image-popup-counter">{state.index + 1} / {total}</div>
          </>
        )}
        <div class="image-popup-strip" ref={stripRef}>
          {state.images.map((imgSrc, i) => (
            <div
              class="image-popup-slide"
              key={imgSrc}
              style={`left: ${shortestDelta(i, state.index, total) * 100}%;`}
            >
              <img src={imgSrc} alt={i === state.index ? 'Full size' : ''} draggable={false} />
            </div>
          ))}
          {/* shortestDelta(_, _, 2) returns 0 or +1 only, never -1, so the
              other image sits only at left:+100%. Mirror it at -100% to
              fill the side a swipe might expose. */}
          {total === 2 && (
            <div class="image-popup-slide" key="mirror" style="left: -100%;">
              <img src={state.images[1 - state.index]} alt="" draggable={false} />
            </div>
          )}
        </div>
      </div>
    </ModalOverlay>
  );
}
