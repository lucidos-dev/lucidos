import { useRef, useEffect, useLayoutEffect, useState } from 'preact/hooks';
import { popupImage, scaledDurationMs } from '../../store/store';
import { CloseIcon, ChevronLeftIcon, ChevronRightIcon, CopyIcon } from './icons';
import { Overlay } from './Overlay';
import {
  fingerDistance,
  fingerMidpoint,
  computePinchUpdate,
  clampPanTransform,
  computeZoomAt,
  fitToWindowScale,
  fullSizeScale,
  naturalImageLayout,
  sameScale,
  steppedScale,
  zoomCeiling,
  zoomFloor,
  zoomPercent,
  type PinchInitial,
  type ImageLayout,
} from '../../utils/pinchGesture';
import { computedTranslateX, isTextInput } from '../../utils/dom';
import { SwipeTouch } from '@lucidos/pane-swipe';
import { copyImageToClipboard } from '../../utils/clipboard';
import { clipboardAbilities } from '../../utils/platform';

const MAX_ZOOM_PAST_FIT = 10;
const WHEEL_FACTOR = 0.002;
const DOUBLE_TAP_SCALE = 3;
const SWIPE_COMMIT_MS = 220;
const SWIPE_SNAP_BACK_MS = 200;
const TRANSITION_SLACK_MS = 180;

const STRIP_REST_TRANSFORM = 'translate3d(0, 0, 0)';

function shortestDelta(i: number, c: number, n: number): number {
  if (n <= 1) return 0;
  let d = ((i - c) % n + n) % n;
  if (d > n / 2) d -= n;
  return d;
}

interface ZoomApi {
  step(direction: 1 | -1): void;
  preset(): void;
  fit(): void;
  refit(): void;
  isZoomed(): boolean;
}

interface ZoomLevel {
  percent: number;
  atFit: boolean;
  atMin: boolean;
  atMax: boolean;
  canPreset: boolean;
}

const FITTED: ZoomLevel =
  { percent: 100, atFit: true, atMin: true, atMax: false, canPreset: true };

function sameLevel(a: ZoomLevel, b: ZoomLevel): boolean {
  return a.percent === b.percent && a.atFit === b.atFit
    && a.atMin === b.atMin && a.atMax === b.atMax && a.canPreset === b.canPreset;
}

function screenPixelRatio(): number {
  return window.devicePixelRatio || 1;
}

function step(delta: -1 | 1) {
  const s = popupImage.value;
  if (!s || s.images.length <= 1) return;
  const next = (s.index + delta + s.images.length) % s.images.length;
  popupImage.value = { images: s.images, index: next };
}

export function ImagePopup() {
  const state = popupImage.value;
  const stripRef = useRef<HTMLDivElement>(null);
  const zoomRef = useRef({ scale: 1, tx: 0, ty: 0 });
  const transformedImgRef = useRef<HTMLImageElement | null>(null);
  const dragRef = useRef({ active: false, startX: 0, startY: 0, originTx: 0, originTy: 0 });
  const pinchRef = useRef<PinchInitial | null>(null);
  const swipeRef = useRef<SwipeTouch | null>(null);
  if (swipeRef.current === null) swipeRef.current = new SwipeTouch();
  const swipe = swipeRef.current;
  const swipeDxRef = useRef<number | null>(null);
  const dragStartRef = useRef<{ offset: number; w: number } | null>(null);
  const layoutRef = useRef<ImageLayout | null>(null);
  const rafRef = useRef<number | null>(null);
  const transitionCleanupRef = useRef<(() => void) | null>(null);
  const pendingCommitRef = useRef<{ from: number; to: number } | null>(null);
  const touchActiveRef = useRef(false);
  const lastStripRef = useRef<HTMLDivElement | null>(null);
  const [chromeHidden, setChromeHidden] = useState(false);
  const [level, setLevel] = useState<ZoomLevel>(FITTED);
  const fittedRef = useRef(true);
  const zoomApiRef = useRef<ZoomApi | null>(null);
  useEffect(() => { if (state) setChromeHidden(false); }, [state !== null]);

  const total = state?.images.length ?? 0;
  const hasNav = total > 1;

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

  useLayoutEffect(() => {
    zoomRef.current = { scale: 1, tx: 0, ty: 0 };
    fittedRef.current = true;
    setLevel(FITTED);
    layoutRef.current = null;
    const old = transformedImgRef.current;
    if (old) {
      old.style.transform = '';
      old.style.cursor = 'zoom-in';
      transformedImgRef.current = null;
    }
  }, [state?.index]);

  useEffect(() => {
    if (!stripRef.current) return;
    const strip: HTMLDivElement = stripRef.current;

    function getCurrentImg(): HTMLImageElement | null {
      const s = popupImage.value;
      if (!s) return null;
      const slide = strip.children[s.index] as HTMLDivElement | undefined;
      return slide?.querySelector<HTMLImageElement>('img') ?? null;
    }

    function captureLayout(): ImageLayout | null {
      const img = getCurrentImg();
      if (!img) return null;
      const slide = img.parentElement;
      if (!slide) return null;
      const z = zoomRef.current;
      return naturalImageLayout(
        slide.getBoundingClientRect(),
        img.getBoundingClientRect(),
        z.scale, z.tx, z.ty,
      );
    }
    function measurable(layout: ImageLayout | null): layout is ImageLayout {
      return layout !== null && layout.imgW > 0 && layout.imgH > 0;
    }
    function ensureLayout(): ImageLayout | null {
      if (layoutRef.current) return layoutRef.current;
      const measured = captureLayout();
      if (!measurable(measured)) return null;
      layoutRef.current = measured;
      return measured;
    }

    function fullScale(): number {
      const img = getCurrentImg();
      const layout = ensureLayout();
      if (!img || !layout) return 0;
      return fullSizeScale(layout.imgW, img.naturalWidth, screenPixelRatio());
    }
    function fitScale(): number {
      const layout = ensureLayout();
      if (!layout) return 1;
      return fitToWindowScale(layout.containerW, layout.containerH, layout.imgW, layout.imgH);
    }
    function zoomRange(fit: number, full: number): { min: number; max: number } {
      return { min: zoomFloor(fit, full), max: zoomCeiling(MAX_ZOOM_PAST_FIT * fit, full) };
    }
    function zoomedPastFit(): boolean {
      const scale = zoomRef.current.scale;
      const fit = fitScale();
      return scale > fit && !sameScale(scale, fit);
    }

    function applyZoom() {
      const img = getCurrentImg();
      if (!img) return;
      const { scale, tx, ty } = zoomRef.current;
      img.style.transform = `translate3d(${tx}px, ${ty}px, 0) scale(${scale})`;
      img.style.cursor = zoomedPastFit() ? 'grab' : 'zoom-in';
      transformedImgRef.current = img;
      const fit = fitScale();
      fittedRef.current = sameScale(scale, fit);
      if (!touchActiveRef.current) publishLevel(fit);
    }
    function publishLevel(fit: number) {
      const scale = zoomRef.current.scale;
      const full = fullScale();
      const range = zoomRange(fit, full);
      const atFit = sameScale(scale, fit);
      const next: ZoomLevel = {
        percent: zoomPercent(scale, full, fit),
        atFit,
        atMin: scale <= range.min || sameScale(scale, range.min),
        atMax: scale >= range.max || sameScale(scale, range.max),
        canPreset: !atFit || full <= 0 || !sameScale(full, fit),
      };
      setLevel(prev => (sameLevel(prev, next) ? prev : next));
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
      const range = zoomRange(fitScale(), fullScale());
      const u = computeZoomAt(
        z.scale, z.tx, z.ty,
        clientX, clientY, newScale,
        layout.natCenterX, layout.natCenterY,
        range.min, range.max,
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
    function zoomToFit() {
      zoomRef.current = { scale: fitScale(), tx: 0, ty: 0 };
      scheduleApplyZoom();
    }

    function stripCenter(): { x: number; y: number } {
      const r = strip.getBoundingClientRect();
      return { x: r.left + r.width / 2, y: r.top + r.height / 2 };
    }
    function stepZoom(direction: 1 | -1) {
      const c = stripCenter();
      const range = zoomRange(fitScale(), fullScale());
      zoomAt(c.x, c.y, steppedScale(zoomRef.current.scale, direction, range.min, range.max));
    }
    function zoomToFullSize() {
      const c = stripCenter();
      zoomAt(c.x, c.y, fullScale() || fitScale() * DOUBLE_TAP_SCALE);
    }
    zoomApiRef.current = {
      step: stepZoom,
      preset: () => {
        if (sameScale(zoomRef.current.scale, fitScale())) zoomToFullSize();
        else zoomToFit();
      },
      fit: zoomToFit,
      refit: () => { if (fittedRef.current) zoomToFit(); },
      isZoomed: zoomedPastFit,
    };

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

    function flushPendingCommit() {
      const pending = pendingCommitRef.current;
      pendingCommitRef.current = null;
      if (!pending) return;
      const cur = popupImage.value;
      if (!cur || cur.index === pending.to) return;
      const w = strip.offsetWidth;
      const tBefore = computedTranslateX(strip);
      const len = cur.images.length;
      const delta = shortestDelta(pending.to, pending.from, len);
      popupImage.value = { images: cur.images, index: pending.to };
      strip.style.transition = 'none';
      strip.style.transform = `translate3d(${delta * w + tBefore}px, 0, 0)`;
    }

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
      const timeoutId = window.setTimeout(finish, scaledDurationMs(SWIPE_COMMIT_MS) + TRANSITION_SLACK_MS);
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
        swipe.cancel();
        cancelSwipeRaf();
        cancelTransition();
        touchActiveRef.current = true;
        flushPendingCommit();
        dragRef.current.active = false;
        dragStartRef.current = null;
        const cur = computedTranslateX(strip);
        strip.style.transition = 'none';
        strip.style.transform = `translate3d(${cur}px, 0, 0)`;
        const layout = captureLayout();
        if (!measurable(layout)) return;
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
      if (zoomedPastFit()) return;
      cancelSwipeRaf();
      cancelTransition();
      touchActiveRef.current = true;
      flushPendingCommit();
      const s = popupImage.value;
      if (!s || s.images.length <= 1) { releaseGesture(); return; }
      const w = strip.offsetWidth;
      const offset = computedTranslateX(strip);
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
        const range = zoomRange(fitScale(), fullScale());
        const u = computePinchUpdate(pinchRef.current, mid.x, mid.y, dist, range.min, range.max);
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
      publishLevel(fitScale());
    }

    function handleTouchEnd(e: TouchEvent) {
      const wasHorizontal = swipe.isHorizontal;
      settleTouch(e, swipe.end(window.innerWidth || 1), wasHorizontal);
    }

    function handleTouchCancel(e: TouchEvent) {
      const wasHorizontal = swipe.isHorizontal;
      swipe.cancel();
      settleTouch(e, 0, wasHorizontal);
    }

    function settleTouch(e: TouchEvent, result: number, wasHorizontal: boolean) {
      const drag = dragStartRef.current;

      if (e.touches.length === 0) layoutRef.current = null;
      pinchRef.current = null;

      if (!drag) {
        if (e.touches.length === 0) releaseGesture();
        return;
      }

      if (result !== 0) {
        cancelSwipeRaf();
        const cur0 = popupImage.value;
        const len = cur0?.images.length ?? 0;
        if (len <= 1) { releaseGesture(); return; }
        const fromIndex = cur0!.index;
        const newIndex = (fromIndex + result + len) % len;
        const targetPx = -result * drag.w;
        strip.style.transition = `transform ${scaledDurationMs(SWIPE_COMMIT_MS)}ms ease-out`;
        strip.style.transform = `translate3d(${targetPx}px, 0, 0)`;
        pendingCommitRef.current = { from: fromIndex, to: newIndex };
        onTransitionDone(() => {
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
        strip.style.transition = `transform ${scaledDurationMs(SWIPE_SNAP_BACK_MS)}ms ease-out`;
        strip.style.transform = STRIP_REST_TRANSFORM;
        onTransitionDone(releaseGesture);
        return;
      }

      releaseGesture();
    }

    function onPointerDown(e: PointerEvent) {
      if (e.button !== 0) return;
      if (pinchRef.current) return;
      if (!zoomedPastFit()) return;
      const z = zoomRef.current;
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
        try { img.setPointerCapture(e.pointerId); } catch {  }
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
        try { img.releasePointerCapture(e.pointerId); } catch {  }
        img.style.cursor = zoomedPastFit() ? 'grab' : 'zoom-in';
      }
    }
    function onDoubleClick(e: MouseEvent) {
      e.preventDefault();
      e.stopPropagation();
      if (zoomedPastFit()) {
        zoomToFit();
      } else {
        zoomAt(e.clientX, e.clientY, fitScale() * DOUBLE_TAP_SCALE);
      }
    }

    function onClick(e: MouseEvent) {
      if (e.detail > 1) return;
      if (zoomedPastFit()) return;
      if (e.target instanceof HTMLImageElement) {
        setChromeHidden(v => !v);
      } else {
        popupImage.value = null;
      }
    }

    function handleResize() {
      if (touchActiveRef.current) return;
      strip.style.transition = 'none';
      strip.style.transform = STRIP_REST_TRANSFORM;
      layoutRef.current = null;
      if (fittedRef.current) {
        zoomToFit();
        return;
      }
      clamp();
      applyZoom();
    }

    strip.addEventListener('wheel', handleWheel, { passive: false });
    strip.addEventListener('touchstart', handleTouchStart, { passive: false });
    strip.addEventListener('touchmove', handleTouchMove, { passive: false });
    strip.addEventListener('touchend', handleTouchEnd, { passive: false });
    strip.addEventListener('touchcancel', handleTouchCancel, { passive: false });
    strip.addEventListener('pointerdown', onPointerDown);
    strip.addEventListener('pointermove', onPointerMove);
    strip.addEventListener('pointerup', onPointerUp);
    strip.addEventListener('pointercancel', onPointerUp);
    strip.addEventListener('dblclick', onDoubleClick);
    strip.addEventListener('click', onClick);
    window.addEventListener('resize', handleResize);
    window.addEventListener('orientationchange', handleResize);
    return () => {
      strip.removeEventListener('wheel', handleWheel);
      strip.removeEventListener('touchstart', handleTouchStart);
      strip.removeEventListener('touchmove', handleTouchMove);
      strip.removeEventListener('touchend', handleTouchEnd);
      strip.removeEventListener('touchcancel', handleTouchCancel);
      strip.removeEventListener('pointerdown', onPointerDown);
      strip.removeEventListener('pointermove', onPointerMove);
      strip.removeEventListener('pointerup', onPointerUp);
      strip.removeEventListener('pointercancel', onPointerUp);
      strip.removeEventListener('dblclick', onDoubleClick);
      strip.removeEventListener('click', onClick);
      window.removeEventListener('resize', handleResize);
      window.removeEventListener('orientationchange', handleResize);
      cancelSwipeRaf();
      cancelTransition();
      pinchRef.current = null;
      dragStartRef.current = null;
      layoutRef.current = null;
      zoomApiRef.current = null;
    };
  }, [state !== null]);

  useEffect(() => {
    zoomApiRef.current?.fit();
  }, [state?.index]);

  useEffect(() => {
    if (!hasNav) return;
    function onKey(e: KeyboardEvent) {
      if (e.key === 'ArrowLeft') { e.preventDefault(); step(-1); }
      else if (e.key === 'ArrowRight') { e.preventDefault(); step(1); }
    }
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [hasNav]);

  useEffect(() => {
    if (state === null) return;
    function onKey(e: KeyboardEvent) {
      if (e.metaKey || e.ctrlKey || e.altKey) return;
      if (isTextInput(e.target)) return;
      if (e.key === '+' || e.key === '=') { e.preventDefault(); zoomApiRef.current?.step(1); }
      else if (e.key === '-') { e.preventDefault(); zoomApiRef.current?.step(-1); }
      else if (e.key === '0') { e.preventDefault(); zoomApiRef.current?.fit(); }
    }
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [state !== null]);

  if (!state) return null;

  function close() {
    popupImage.value = null;
  }

  return (
    <Overlay
      open
      onClose={() => {
        const zoom = zoomApiRef.current;
        if (zoom?.isZoomed()) { zoom.fit(); return false; }
        close();
      }}
      overlayClass="image-popup"
      panelClass={`image-popup-content${chromeHidden ? ' chrome-hidden' : ''}`}
    >
        <button class="image-popup-close icon-btn" onClick={close} aria-label="Close" data-tooltip="Close">
          <CloseIcon />
        </button>
        <button class="floating-mobile-close" onClick={close} aria-label="Close">
          <CloseIcon />
        </button>
        {clipboardAbilities().copyImage && (
          <button
            class="image-popup-copy icon-btn"
            onClick={() => copyImageToClipboard(state.images[state.index])}
            aria-label="Copy image"
            data-tooltip="Copy image"
          >
            <CopyIcon />
          </button>
        )}
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
        <div class="image-popup-zoom" role="group" aria-label="Zoom">
          <button
            class="image-popup-zoom-btn"
            onClick={() => zoomApiRef.current?.step(-1)}
            disabled={level.atMin}
            aria-label="Zoom out"
            data-tooltip="Zoom out"
          >
            −
          </button>
          <button
            class="image-popup-zoom-btn image-popup-zoom-level"
            onClick={() => zoomApiRef.current?.preset()}
            disabled={!level.canPreset}
            aria-label={level.canPreset
              ? `${level.percent}%, ${level.atFit ? 'zoom to actual size' : 'fit to window'}`
              : `${level.percent}%`}
            data-tooltip={level.canPreset
              ? (level.atFit ? 'Zoom to actual size' : 'Fit to window')
              : undefined}
          >
            {level.percent}%
          </button>
          <button
            class="image-popup-zoom-btn"
            onClick={() => zoomApiRef.current?.step(1)}
            disabled={level.atMax}
            aria-label="Zoom in"
            data-tooltip="Zoom in"
          >
            +
          </button>
        </div>
        <div class="image-popup-strip" ref={stripRef}>
          {state.images.map((imgSrc, i) => (
            <div
              class="image-popup-slide"
              key={imgSrc}
              style={`left: ${shortestDelta(i, state.index, total) * 100}%;`}
            >
              <img
                src={imgSrc}
                alt={i === state.index ? 'Full size' : ''}
                draggable={false}
                onLoad={() => zoomApiRef.current?.refit()}
              />
            </div>
          ))}
          {total === 2 && (
            <div class="image-popup-slide" key="mirror" style="left: -100%;">
              <img src={state.images[1 - state.index]} alt="" draggable={false} />
            </div>
          )}
        </div>
    </Overlay>
  );
}
