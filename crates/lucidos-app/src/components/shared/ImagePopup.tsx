import { useRef, useEffect, useLayoutEffect, useState } from 'preact/hooks';
import { popupImage } from '../../store/store';
import { CloseIcon, ChevronLeftIcon, ChevronRightIcon } from './icons';
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
import { isTextInput } from '../../utils/dom';
import { SwipeTouch } from '../../utils/swipe';

// How far past the fitted view a gesture zooms, raised per image so full size
// always fits under it (see `zoomRange` below).
const MAX_ZOOM_PAST_FIT = 10;
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

/** What the zoom cluster and the zoom keys can do. The gesture effect owns the
 *  geometry these need, so it publishes them rather than letting the render
 *  tree reach into the transform itself. */
interface ZoomApi {
  /** One press of zoom in (`1`) or zoom out (`-1`), anchored at the centre. */
  step(direction: 1 | -1): void;
  /** Full size from the fitted view, back to fitted from anywhere else. */
  preset(): void;
  /** Back to the fitted view. */
  fit(): void;
  /** Fit again, but only where that would not throw away a chosen zoom. */
  refit(): void;
  /** Whether the image is blown up past its fitted size. */
  isZoomed(): boolean;
}

/** What the zoom cluster reads out: the live level, and whether each control
 *  still has somewhere to go. */
interface ZoomLevel {
  percent: number;
  atFit: boolean;
  atMin: boolean;
  atMax: boolean;
  /** Whether the fit / actual-size toggle has a second place to go. An image
   *  that fills the screen it was captured on is already at 1:1, so both ends
   *  of the toggle are the same place. */
  canPreset: boolean;
}

// Where every image starts, and what an unmeasured one reads as.
const FITTED: ZoomLevel =
  { percent: 100, atFit: true, atMin: true, atMax: false, canPreset: true };

function sameLevel(a: ZoomLevel, b: ZoomLevel): boolean {
  return a.percent === b.percent && a.atFit === b.atFit
    && a.atMin === b.atMin && a.atMax === b.atMax && a.canPreset === b.canPreset;
}

// Device pixels the screen draws per CSS pixel, which is what makes 100% mean
// one image pixel per screen pixel. Read live: it changes with browser zoom and
// with a move to another display, and `handleResize` re-fits off the new value.
function screenPixelRatio(): number {
  return window.devicePixelRatio || 1;
}

function step(delta: -1 | 1) {
  const s = popupImage.value;
  if (!s || s.images.length <= 1) return;
  const next = (s.index + delta + s.images.length) % s.images.length;
  popupImage.value = { images: s.images, index: next };
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
  // The image we last wrote a transform onto. Whoever wrote one owns clearing
  // it: the geometry is recovered by dividing the live transform back out of a
  // measured rect, and that only holds while the element and `zoomRef` agree.
  // A slide left carrying the fit of a previous visit measures 4.5x its own
  // size, and the fit computed from that is nonsense.
  const transformedImgRef = useRef<HTMLImageElement | null>(null);
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
  const layoutRef = useRef<ImageLayout | null>(null);
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
  // What the level control shows. This is the only part of a zoom the render
  // tree cares about.
  const [level, setLevel] = useState<ZoomLevel>(FITTED);
  // True while the image rests at its fitted scale. A fit is a measurement, so
  // it has to be taken again once the image loads or the window changes size.
  // This says whether doing so would overwrite a zoom the user chose.
  const fittedRef = useRef(true);
  // Published by the gesture effect below, which owns the geometry the zoom
  // controls act on. Null while the popup is closed.
  const zoomApiRef = useRef<ZoomApi | null>(null);
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

  // Reset zoom when navigating to a new image. The fitted scale for the image
  // arriving is a measurement of it, so the effect below takes it once the new
  // slide has rendered.
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

    function captureLayout(): ImageLayout | null {
      const img = getCurrentImg();
      if (!img) return null;
      const slide = img.parentElement;
      if (!slide) return null;
      // imgRect reflects the live transform; naturalImageLayout divides the
      // current zoom (read from zoomRef, the source of truth we apply) back out
      // so the captured geometry is always the natural scale-1 layout — even
      // when a gesture starts on an already-zoomed image.
      const z = zoomRef.current;
      return naturalImageLayout(
        slide.getBoundingClientRect(),
        img.getBoundingClientRect(),
        z.scale, z.tx, z.ty,
      );
    }
    // An image that has not painted yet measures as a zero box, which is
    // nothing to zoom or clamp against. Caching one would freeze every later
    // gesture against an image of no size, so a caller waits instead.
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

    // Full size for the image on screen: the scale where one image pixel covers
    // one physical screen pixel. `0` means the image declares no intrinsic
    // width, so there is no 1:1 to aim at and the plain range stands.
    function fullScale(): number {
      const img = getCurrentImg();
      const layout = ensureLayout();
      if (!img || !layout) return 0;
      return fullSizeScale(layout.imgW, img.naturalWidth, screenPixelRatio());
    }
    // Where the image rests: touching the window edge, whatever its own size.
    // An image smaller than the window is blown up to reach it, which is what
    // makes this a fit rather than a cap.
    function fitScale(): number {
      const layout = ensureLayout();
      if (!layout) return 1;
      return fitToWindowScale(layout.containerW, layout.containerH, layout.imgW, layout.imgH);
    }
    // The band every zoom moves in. Both ends are widened per image to keep
    // full size reachable, whichever side of the fitted view it falls on. It
    // takes its two measurements rather than reading them, so a caller holding
    // one already cannot end up comparing against a second reading of it.
    function zoomRange(fit: number, full: number): { min: number; max: number } {
      return { min: zoomFloor(fit, full), max: zoomCeiling(MAX_ZOOM_PAST_FIT * fit, full) };
    }
    // Blown up past the fitted view, so there is something to pan around and a
    // click means something other than a dismiss.
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
      // The control label is the one reactive part of a zoom, and a pinch runs
      // this every frame. Writing state there would re-render the popup on each
      // finger move, so the gesture's own release publishes the final value.
      if (!touchActiveRef.current) publishLevel(fit);
    }
    // The level the cluster reads out. Identity is compared away, so a pan or a
    // scale change too small to show costs no render.
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
        // Fitted, and the fit already is 1:1. The toggle's other end is here.
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

    // The zoom controls act on the middle of the visible image. A wheel or a
    // double tap points at a spot itself; a button press has no other anchor.
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
      // Nothing to hit 1:1 against on an image with no intrinsic width: land on
      // the double-tap step, so the control still does something predictable.
      zoomAt(c.x, c.y, fullScale() || fitScale() * DOUBLE_TAP_SCALE);
    }
    zoomApiRef.current = {
      step: stepZoom,
      // The standard toggle: 1:1 from the fitted view, and back to fitted from
      // anywhere else.
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
        // Captured fresh rather than through ensureLayout: the strip may be
        // mid-animation, and this gesture's geometry is where it is NOW.
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
      if (zoomedPastFit()) return;  // pointer-pan handles zoomed drag
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
      // The frames a pinch ran through skipped this (see applyZoom), so the
      // level control catches up with where the fingers left the image.
      publishLevel(fitScale());
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
      // detail>1 is the second click of a dblclick; skip so we don't
      // toggle twice on the way to dblclick-zoom.
      if (e.detail > 1) return;
      if (zoomedPastFit()) return;
      // Backdrop clicks close — the modal-overlay handles the outer
      // padding, the strip handles the dark area inside content.
      if (e.target instanceof HTMLImageElement) {
        setChromeHidden(v => !v);
      } else {
        popupImage.value = null;
      }
    }

    // Viewport resize / orientation change. Snap the strip back to rest (slides
    // reposition via render) and drop the now-stale cached layout. A fitted
    // view is measured against the window, so it is measured again. A zoom the
    // user chose is kept, and only pulled back inside the new bounds. Without
    // the re-clamp, rotating the device while zoomed would leave the image
    // panned outside its new edges. Skipped mid-gesture so it can't fight an
    // in-flight pinch or swipe.
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
    strip.addEventListener('touchcancel', handleTouchEnd, { passive: false });
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
      strip.removeEventListener('touchcancel', handleTouchEnd);
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
    // ImagePopup is always mounted at App root; the strip DOM only exists
    // when state is non-null. Re-run on open/close transitions so listeners
    // attach to the freshly-mounted strip.
  }, [state !== null]);

  // Fit the image the popup just landed on. Declared after the gesture effect
  // so the API it calls is already published. An image still downloading has
  // nothing to measure, and asks again from its own load event below.
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

  // The zoom keys every viewer has, mirroring the control cluster. A modified
  // chord is left alone, so the browser's own zoom still works.
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
    // While zoomed, a dismiss backs out ONE step: the image drops to its fitted
    // size and the popup stays open. Returning false says so, and the dismiss
    // hook then neither closes nor swallows the paired click. Escape arrives
    // here too, through the overlay stack, so it unzooms and then closes rather
    // than doing nothing at all. The strip's own click handler still closes and
    // toggles the chrome for clicks inside the panel.
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
        {/* Zoom, for the pointer and the keyboard. A wheel and a pinch are
            invisible affordances, and an image fitted to the window can be far
            too small to read. The middle control reads out the level, as a
            percentage of the image's own pixels against the screen's, and
            toggles fit against 1:1. Its label reads the level and then names
            the action, both as verbs: "450%, zoom to actual size" says what a
            press does, where a bare "450%, actual size" claims the image is
            already at 1:1. It greys out where the two ends coincide, which is
            an image fitting the screen it came from, and there the label is the
            level alone. */}
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
              {/* A fit is a measurement, and there is nothing to measure until
                  the image has arrived. Whichever slide that is, the fit taken
                  here is the current one's, and only where the view still
                  rests on a fit. */}
              <img
                src={imgSrc}
                alt={i === state.index ? 'Full size' : ''}
                draggable={false}
                onLoad={() => zoomApiRef.current?.refit()}
              />
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
    </Overlay>
  );
}
