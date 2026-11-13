import { useRef, useEffect, useLayoutEffect } from 'preact/hooks';
import { mobileView, MOBILE_VIEWS, PANE_INDEX, PANE_COUNT, appPseudoFullscreen, type MobileView } from '../../store/store';
import { swipeToNeighbourPane } from '../../store/actions/pane';
import { MOBILE_PANE_CONFIGS } from './MobileAppHeader';
import { DashboardPanel } from '../dashboard/DashboardPanel';
import { EdgeSwipeZones } from './EdgeSwipeZones';
import { documentReplayHost, replayEdgeTap } from './edgeTapReplay';
import {
  isInteractiveTarget,
  opensSoftwareKeyboard,
  getRemPx,
  viewportIsKeyboardShrunk,
  readableVisualViewport,
  computedTranslateX,
} from '../../utils/dom';
import { PaneDrag, attachPaneDragTouches, isFrameInside, type PaneTrack, type TouchPoint } from './paneDrag';
import { subscribeFrameSwipes } from '../../store/actions/frame-swipe-bridge';
import { nativeFullscreenElement } from '../../store/appFullscreenHost';
import {
  noteKeyboardClosed,
  notePolledViewport,
  noteViewportResize,
  noteWakeViewport,
} from './keyboardCloseRelayout';
import { onPerfEnabledChange, perfRecordingOn } from '../../utils/perfQueue';
import { reportNavigation } from '../../utils/navigationMarks';
import { useFocusedFieldVisible } from '../../hooks/useFocusedFieldVisible';

// Rubber band factor at edges (0 = no movement, 1 = full movement)
export const RUBBER_BAND = 0.3;

/** Width of the LEFT screen-edge strip treated as a potential iOS *back*
 *  navigation swipe and suppressed, in REM. It must cover the whole
 *  `.edge-swipe-left` zone, which mobile.css authors at this same 2.5rem.
 *
 *  Over an app iframe, a back-swipe from the screen edge lands on that zone,
 *  since the zone sits above the frame. A guard narrower than the zone leaves a
 *  band that reaches the in-app handler with no preventDefault(). WebKit's
 *  native pop then takes the PWA out to the workspace gateway picker.
 *
 *  Rem and not px, because the mobile breakpoint puts the root at 112.5% and
 *  the UI scale moves it again across 75 to 200%. A px literal is right at
 *  exactly one of those roots, and wrong at the rest: too narrow leaks the band
 *  above, too wide eats vertical scrolling on ordinary content. iOS activates
 *  its interactive pop from roughly the outermost 20pt, inside the zone at
 *  every scale. */
export const EDGE_NAV_GUARD_LEFT_REM = 2.5;

/** Width of the RIGHT screen-edge strip, the *forward* navigation gesture, in
 *  REM. Covers the `.edge-swipe-right` zone, which mobile.css authors at this
 *  same 1.25rem. Narrower than the left strip, so it stays clear of content and
 *  scrolling. */
export const EDGE_NAV_GUARD_RIGHT_REM = 1.25;

/** Pure decision: should a touchstart at `clientX` call preventDefault() to
 *  suppress iOS's native back/forward navigation swipe?
 *
 *  A standalone iOS PWA exposes NO CSS/touch-action opt-out for this gesture,
 *  and WebKit's edge recognizer commits before our in-app 8px horizontal lock
 *  (SwipeTouch), so preventing the default on a touchmove runs too late. The
 *  only reliable suppression is preventDefault on the touchstart itself. Scoped
 *  to the screen-edge strips (sized to the `.edge-swipe-*` zones) and to
 *  non-interactive, non-text-input targets so taps on edge controls and
 *  vertical scrolling elsewhere survive. A tap on any other edge target
 *  loses its click, which `replayEdgeTap` restores.
 *
 *  `remPx` is handed in rather than read here, so the edge math stays testable
 *  without a DOM. The caller measures it per touch, which is what keeps both
 *  guards on their zones when the user moves the UI scale. */
export function shouldSuppressEdgeNavigation(args: {
  clientX: number;
  viewportWidth: number;
  remPx: number;
  targetIsInteractive: boolean;
  textInputFocused: boolean;
}): boolean {
  const { clientX, viewportWidth, remPx, targetIsInteractive, textInputFocused } = args;
  if (targetIsInteractive || textInputFocused) return false;
  return clientX <= EDGE_NAV_GUARD_LEFT_REM * remPx
    || clientX >= viewportWidth - EDGE_NAV_GUARD_RIGHT_REM * remPx;
}

/** Pure decision: may a Lucidos pane swipe START for this touch?
 *
 *  The first two are properties of the touch: a focused text input means the
 *  user is typing and a horizontal drag must not navigate away, and a
 *  horizontally-scrollable target (a code block, a range slider knob) owns its
 *  own horizontal drag.
 *
 *  `appFullscreen` is the odd one out, being a property of the app rather than
 *  the touch: a pseudo-fullscreen app IS the screen, so the three-pane swipe is
 *  off entirely. Nothing visible moves during it anyway (the overlay is
 *  position:fixed over the viewport and the track's transform is pinned to
 *  `none`), so a pane change there is invisible state drift, and the user
 *  leaves fullscreen on a pane they never chose. Deliberately does NOT cover
 *  the separate suppression of WebKit's native edge gesture, which must keep
 *  running while fullscreen: see `shouldSuppressEdgeNavigation`. */
export function shouldStartPaneSwipe(args: {
  textInputFocused: boolean;
  targetScrollable: boolean;
  appFullscreen: boolean;
}): boolean {
  const { textInputFocused, targetScrollable, appFullscreen } = args;
  return !textInputFocused && !targetScrollable && !appFullscreen;
}

/** Whether `pane` is off screen, which pauses every animation inside it
 *  (mobile.css). It follows the signal, not the track. A pane revealed by a
 *  finger drag stays paused until the release makes it current, and then it
 *  animates through the slide. */
export function isOffscreenPane(pane: MobileView, current: MobileView): boolean {
  return pane !== current;
}

/** Pure decision: does focus on `el` mean the mobile keyboard is up over the
 *  app, i.e. should `data-keyboard-active` be set?
 *
 *  Scoped to `<textarea>` (not every text input) so the header search bar stays
 *  interactive while its `<input>` is focused.
 *
 *  Takes the element rather than reading `document.activeElement` itself, so the
 *  same predicate answers for a focus EVENT's target and for the live focus (see
 *  `reconcileKeyboardActive`). Duck-typed on `tagName` rather than `instanceof`,
 *  matching `shouldSuppressDragStart`: testable without a DOM, and
 *  realm-agnostic. */
export function isKeyboardActiveTarget(el: EventTarget | null): boolean {
  return (el as { tagName?: string } | null)?.tagName === 'TEXTAREA';
}

/** Attribute sink: `<html>` in the app, a recording stub in tests. */
interface AttrTarget {
  setAttribute(name: string, value: string): void;
  removeAttribute(name: string): void;
}

/** Re-derive `data-keyboard-active` from the LIVE focus, rather than from the
 *  last focus event seen.
 *
 *  The focusin/focusout pair below describes focus by its TRANSITIONS, and one
 *  transition is never reported: removing the focused element from the DOM moves
 *  focus to `<body>` WITHOUT firing focusout, in WebKit and Chromium alike
 *  (verified on mobile WebKit). The flag then outlives the keyboard that
 *  justified it, and everything it gates stays inert with nothing focused and no
 *  event left to clear it. An iOS PWA resume is the same hazard from the other
 *  side: the page can come back with the attribute intact and focus dropped.
 *
 *  Idempotent, so it is safe to run on every touch. */
export function reconcileKeyboardActive(root: AttrTarget, active: EventTarget | null): void {
  if (isKeyboardActiveTarget(active)) root.setAttribute('data-keyboard-active', '');
  else root.removeAttribute('data-keyboard-active');
}

/** How long after a focus the keyboard may still be rising, in ms. */
export const KEYBOARD_RISE_GRACE_MS = 1000;

/** Pure decision: is this *ghost focus*, a keyboard field focused with the
 *  keyboard down?
 *
 *  The iOS photo picker leaves the composer that way, and so does a focus()
 *  made outside a gesture. Everything that reads focus as typing then holds:
 *  the header stays away and pane swipes are refused. iOS keeps the focus
 *  through a tap on anything unfocusable, so the user has no way out.
 *
 *  A hardware keyboard also leaves the viewport unshrunk. A key pressed with
 *  the viewport unshrunk proves one, so that focus is real typing. */
export function isGhostFocus(args: {
  fieldFocused: boolean;
  keyboardShrunk: boolean;
  focusHeldMs: number;
  hardwareKeyPressed: boolean;
}): boolean {
  const { fieldFocused, keyboardShrunk, focusHeldMs, hardwareKeyPressed } = args;
  return fieldFocused && !keyboardShrunk && !hardwareKeyPressed
    && focusHeldMs >= KEYBOARD_RISE_GRACE_MS;
}

/** Pure decision: what value should `--app-height` be written to, given the
 *  current visual viewport, layout viewport, and focus state? Extracted so
 *  the wake/resize bug can be unit-tested without jsdom + fake visualViewport.
 *
 *  The shell is `innerHeight` (the layout viewport, which the iOS keyboard
 *  does not shrink) minus `keyboardBandPx`, which says when the band is
 *  non-zero. With no field focused it is 0, whatever `vv.height` says. That
 *  fixes the iOS PWA wake bug, where a delayed `vv.resize` fires with a stale
 *  shrunk `vv.height` even though the keyboard is actually dismissed.
 *  The companion fix is in `onWake`, which blurs any iOS-preserved focus on
 *  a text input so the `activeElementOpensKeyboard` signal correctly reports
 *  false when the keyboard is actually down. */
export function computeAppHeight(args: KeyboardReading): number {
  return args.innerHeight - keyboardBandPx(args);
}

/** What the shell reads to decide where the keyboard is. */
interface KeyboardReading {
  vvHeight: number;
  innerHeight: number;
  activeElementOpensKeyboard: boolean;
  /** The band to assume when the visual viewport is *blind*, and 0 when it
   *  sees. See `trackAppHeight`. */
  blindBandPx?: number;
}

/** Pure decision: how much room the software keyboard is taking off the foot of
 *  the screen, in px, and 0 when it is down.
 *
 *  Published as `--keyboard-band` and spent as bottom padding inside the pane's
 *  scroll container. iOS reveals a focused field by scrolling its nearest
 *  scrollable ancestor. When that ancestor cannot scroll far enough it offsets
 *  the whole viewport instead, which takes the fixed header with it. It also
 *  moves the frame every measurement is made in, and that is what made the
 *  reveal land differently on the same form twice running. The padding is the
 *  slack that stops iOS reaching for the offset.
 *
 *  `computeAppHeight` subtracts this band, so the shell's height and the slack
 *  it needs cannot disagree about whether the keyboard is up.
 *  See `docs/plans/2026-09-19-the-keyboard-reveal-stops-fighting-ios.md`. */
export function keyboardBandPx(args: KeyboardReading): number {
  const { vvHeight, innerHeight, activeElementOpensKeyboard, blindBandPx = 0 } = args;
  if (!activeElementOpensKeyboard) return 0;
  if (viewportIsKeyboardShrunk(vvHeight, innerHeight)) return innerHeight - vvHeight;
  return blindBandPx;
}

/** Pure decision: the padding a scroller must keep when the band drops, in px,
 *  and 0 when the band alone is enough. This is the *band hold*.
 *
 *  A reader scrolled into the band would otherwise lose the padding under
 *  them. The browser clamps their scrollTop and the content jumps, so a press
 *  on a button lands its release on whatever moved there. The hold is the
 *  least padding that keeps scrollTop valid, and it never grows back, so it
 *  drains as the reader scrolls toward the content. It lets go at once when
 *  the content it anchored shrinks, since that content is no longer there.
 *  See `docs/plans/2026-09-24-the-keyboard-band-holds-the-scroll-anchor.md`. */
export function heldBandPx(args: {
  paddingPx: number;
  scrollTop: number;
  scrollHeight: number;
  clientHeight: number;
  bandPx: number;
  heldPx?: number;
  anchorContentPx?: number;
}): number {
  const { paddingPx, scrollTop, scrollHeight, clientHeight, bandPx, heldPx = Infinity, anchorContentPx = 0 } = args;
  // A pixel of slack, since scrollHeight is whole and the padding need not be.
  if (scrollTop <= 0 || scrollHeight - paddingPx < anchorContentPx - 1) return 0;
  const room = scrollHeight - clientHeight - scrollTop;
  const needed = Math.min(heldPx, Math.round(paddingPx - room));
  return needed > bandPx ? needed : 0;
}

/** The band to assume on a focus before any keyboard has been measured.
 *  An iPhone's portrait keyboard is a little under half the screen, and
 *  over-reserving costs only scrollable slack nobody scrolls into. */
const ASSUMED_KEYBOARD_FRACTION = 0.45;

/** Where the last measured band per orientation is kept, for a launch whose
 *  visual viewport is blind from the start. */
export const KEYBOARD_BAND_STORAGE_KEY = 'lucidos-keyboard-band';

type Orientation = 'portrait' | 'landscape';

/** The stored bands, skipping any entry that is not a positive number.
 *
 *  Storage failures only warn. The memory runs without user intent, so a toast
 *  would report nothing the user did. A miss falls back to the assumed band,
 *  and the next measured keyboard rewrites the stored copy. */
function loadStoredBands(storage: Pick<Storage, 'getItem'>): Record<Orientation, number> {
  const bands = { portrait: 0, landscape: 0 };
  try {
    const parsed: unknown = JSON.parse(storage.getItem(KEYBOARD_BAND_STORAGE_KEY) ?? 'null');
    if (typeof parsed !== 'object' || parsed === null) return bands;
    for (const key of ['portrait', 'landscape'] as const) {
      const px = (parsed as Record<string, unknown>)[key];
      if (typeof px === 'number' && Number.isFinite(px) && px > 0) bands[key] = px;
    }
  } catch (err) {
    console.warn('Keyboard band memory: ignoring unreadable stored bands', err);
  }
  return bands;
}

/** The band to reserve per orientation: the widest seen this session, seeded
 *  from the last one stored. A focus uses it to reserve the slack
 *  before the keys have animated in. iOS picks between scrolling the container
 *  and offsetting the viewport at FOCUS time. Slack arriving with the first
 *  resize arrives too late to change that decision.
 *
 *  Kept per orientation, since the phone layout survives a rotation. A
 *  portrait keyboard is most of a landscape screen.
 *
 *  Given storage, it starts from the last band an earlier launch measured, and
 *  stores each new measurement. The latest is stored, not the widest, so a
 *  keyboard that got shorter is not over-reserved forever. */
export function keyboardBandMemory(storage?: Pick<Storage, 'getItem' | 'setItem'>) {
  const widest = storage ? loadStoredBands(storage) : { portrait: 0, landscape: 0 };
  const stored = { ...widest };
  const orientationOf = (v: { width: number; height: number }): Orientation =>
    (v.width > v.height ? 'landscape' : 'portrait');
  return {
    note(bandPx: number, viewport: { width: number; height: number }) {
      const key = orientationOf(viewport);
      if (bandPx > widest[key]) widest[key] = bandPx;
      if (!storage || bandPx <= 0 || bandPx === stored[key]) return;
      stored[key] = bandPx;
      try {
        storage.setItem(KEYBOARD_BAND_STORAGE_KEY, JSON.stringify(stored));
      } catch (err) {
        console.warn('Keyboard band memory: could not store the measured band', err);
      }
    },
    reserve(viewport: { width: number; height: number }): number {
      return widest[orientationOf(viewport)] || Math.round(viewport.height * ASSUMED_KEYBOARD_FRACTION);
    },
  };
}

/** How often the gated viewport poll reads the keyboard's state.
 *  The composer probe's scheduled cadence, which the ledger shows still
 *  running throughout a wedge. */
const KEYBOARD_POLL_MS = 3000;

/** Check if an element or any ancestor (up to pane boundary) scrolls horizontally. */
function isHorizontallyScrollable(el: Element | null): boolean {
  while (el) {
    if (el.classList.contains('mobile-swipe-pane')) break;
    if (el.scrollWidth > el.clientWidth) {
      const style = getComputedStyle(el);
      if (style.overflowX === 'auto' || style.overflowX === 'scroll') return true;
    }
    el = el.parentElement;
  }
  return false;
}

/** The strip's offset for a pane. Px once the container has a width, and
 *  the track's own percentage before (300% of the container, so pane 1 is
 *  -33.333%). */
function paneTransform(index: number, paneWidth: number): string {
  return paneWidth > 0 ? `translateX(${-index * paneWidth}px)` : `translateX(${-index * 100 / PANE_COUNT}%)`;
}

/** A held offset, rubber-banded past either end of the strip. */
export function rubberBanded(offset: number, paneWidth: number): number {
  const minOffset = -(PANE_COUNT - 1) * paneWidth;
  if (offset > 0) return offset * RUBBER_BAND;
  if (offset < minOffset) return minOffset + (offset - minOffset) * RUBBER_BAND;
  return offset;
}

/** The strip as a `PaneTrack`: the only code that writes its transform and
 *  transition. A drag holds it with the transition off. Otherwise it rests on,
 *  or slides to, the visible pane with the transition on. */
export function stripTrack(
  containerRef: { current: HTMLElement | null },
  trackRef: { current: HTMLElement | null },
  isHeld: () => boolean,
): PaneTrack {
  // Read once at the grab: a resize ends the drag, so the width holds.
  let base = 0;
  let heldWidth = 0;
  const paneWidth = () => containerRef.current?.offsetWidth ?? 0;
  return {
    paneWidth,
    grab() {
      const track = trackRef.current;
      if (!track) return;
      // Where it is on screen, so a drag that locks mid-slide catches it there.
      base = computedTranslateX(track);
      heldWidth = paneWidth();
      track.style.transition = 'none';
    },
    follow(dx) {
      const track = trackRef.current;
      if (!track) return;
      track.style.transition = 'none';
      track.style.transform = `translateX(${rubberBanded(base + dx, heldWidth)}px)`;
    },
    navigate: swipeToNeighbourPane,
    rest(animate) {
      const container = containerRef.current;
      const track = trackRef.current;
      if (!container || !track) return;
      // The strip is `overflow: hidden` (ADR 0420), so a focus() may have
      // scrolled it. Resting puts the panes back on their transform.
      container.scrollLeft = 0;
      track.style.transform = paneTransform(PANE_INDEX[mobileView.value], container.offsetWidth);
      if (animate) {
        track.style.transition = '';
        return;
      }
      track.style.transition = 'none';
      // Flush, so the snap lands before the next frame turns the transition
      // back on. Without it, that frame's style pass animates the snap.
      void track.offsetWidth;
      requestAnimationFrame(() => {
        if (!isHeld() && trackRef.current) trackRef.current.style.transition = '';
      });
    },
  };
}

/** Whether a touch on `target` drives a horizontal gesture of its own, which a
 *  pane swipe must not hijack: a horizontally scrollable child (a code block),
 *  a range slider's knob, or a dashboard tile dragged in edit mode. */
export function targetOwnsHorizontalDrag(target: Element): boolean {
  return isHorizontallyScrollable(target) || !!target.closest('input[type="range"], .dashboard-tile-shield');
}

/** May a pane swipe start for this touch on the strip? Also suppresses
 *  iOS's native back/forward swipe at the screen edges, for every new touch.
 *
 *  The suppression must happen on touchstart, because WebKit's edge
 *  recognizer commits before our 8px horizontal lock. It reads the touch that
 *  started, never an older one. So a thumb resting on an edge swipe zone
 *  does not cancel every other touch's scroll. Scrollable children,
 *  interactive controls and focused text inputs are exempt, so their own
 *  gestures and taps survive (see shouldSuppressEdgeNavigation). A tap on any
 *  other edge target loses its click to the cancel, so `replayEdgeTap` gives it
 *  back to the element under the strip. */
function gateStripTouch(e: TouchEvent, touch: TouchPoint): boolean {
  const target = e.target as Element;
  const textInputFocused = opensSoftwareKeyboard(document.activeElement);
  const targetScrollable = targetOwnsHorizontalDrag(target);

  if (!targetScrollable && shouldSuppressEdgeNavigation({
    clientX: touch.clientX,
    viewportWidth: window.innerWidth,
    // Read per touch, never captured at mount: the guards track the rem zones
    // through a UI-scale change the user makes with the app open.
    remPx: getRemPx(),
    targetIsInteractive: isInteractiveTarget(target),
    textInputFocused,
  })) {
    e.preventDefault();
    // The cancel also kills the click a tap on this strip would have paired.
    // Only a cancel that took is replayed. A touchstart that is not cancelable
    // keeps its native click, or has none when it stops a fling.
    if (e.defaultPrevented) replayEdgeTap(documentReplayHost(), e);
  }

  // Must come AFTER the suppression above: a pseudo-fullscreen app cancels
  // OUR swipe, but WebKit's native one still has to be preventDefault'd or
  // suppressing ours just hands the gesture over. See shouldStartPaneSwipe.
  return shouldStartPaneSwipe({
    textInputFocused,
    targetScrollable,
    appFullscreen: appPseudoFullscreen.value,
  });
}

/** A natively fullscreen app counts too. A host touch can never reach the
 *  container then, but a frame drag can. */
function appIsFullscreen(): boolean {
  return appPseudoFullscreen.value || nativeFullscreenElement() !== null;
}

/** Own `--app-height` and `--keyboard-band` for the mobile shell, and return
 *  the teardown. Exported so a test can drive it against a fake viewport. */
export function trackAppHeight(): (() => void) | undefined {
  const vv = window.visualViewport;
  if (!vv) return;
  // -1 sentinel ensures the initial setHeight() call writes the CSS variable
  // instead of being short-circuited by the equality guard inside setHeight.
  let lastSetHeight = -1;

  // px, not rem: --app-height is a physical viewport measurement that must
  // NOT scale with --user-ui-scale. Using rem caused the app shell to exceed
  // the viewport when applyUiScale() changed the base font-size after mount.
  const setHeight = (h: number) => {
    if (h === lastSetHeight) return;
    lastSetHeight = h;
    document.documentElement.style.setProperty('--app-height', `${h}px`);
  };

  const bandMemory = keyboardBandMemory(window.localStorage);
  const layoutViewport = () => ({ width: window.innerWidth, height: window.innerHeight });

  // The visual viewport is *blind* when it reads zero before its first real
  // reading, until it next measures a keyboard. iOS can launch the PWA that
  // way and keep the full height with the keys up until a relaunch. Focus
  // then decides alone, and the band is the remembered keyboard. A zero after
  // a real reading is a wake's transient one and leaves this alone: with a
  // hardware keyboard no measurement would ever clear it. Registered in
  // `docs/temporary-measures.md` § 1, "Blind visual viewport".
  let viewportBlind = false;
  let viewportHasRead = false;
  // One reading site, so every write makes the same keyboard-aware decision.
  // A rotation or mount with the keys up must keep the shrunk height. Writing
  // innerHeight blindly would put the prompt behind the keys.
  const keyboardReading = (): KeyboardReading => {
    const activeElementOpensKeyboard = opensSoftwareKeyboard(document.activeElement);
    if (!readableVisualViewport()) {
      if (!viewportHasRead) viewportBlind = true;
    } else {
      viewportHasRead = true;
      if (viewportIsKeyboardShrunk(vv.height, window.innerHeight)) {
        viewportBlind = false;
        if (activeElementOpensKeyboard) bandMemory.note(window.innerHeight - vv.height, layoutViewport());
      }
    }
    return {
      vvHeight: vv.height,
      innerHeight: window.innerHeight,
      activeElementOpensKeyboard,
      blindBandPx: viewportBlind ? bandMemory.reserve(layoutViewport()) : 0,
    };
  };
  const layoutViewportKey = () => `${window.innerWidth}x${window.innerHeight}`;
  let measuredLayoutViewport = '';
  const currentAppHeight = () => {
    measuredLayoutViewport = layoutViewportKey();
    return computeAppHeight(keyboardReading());
  };
  let lastSetBand = -1;
  // Each scroller's band hold (`heldBandPx`): the content height it anchors,
  // and the scroll listener and mutation observer that re-measure it.
  interface Hold { px: number; anchorContentPx: number; recheck: () => void; observer: MutationObserver }
  const holds = new Map<HTMLElement, Hold>();
  const releaseHold = (el: HTMLElement) => {
    const hold = holds.get(el);
    if (!hold) return;
    el.removeEventListener('scroll', hold.recheck);
    hold.observer.disconnect();
    el.style.removeProperty('--keyboard-band-hold');
    holds.delete(el);
  };
  const updateHold = (el: HTMLElement, bandPx: number) => {
    const paddingPx = parseFloat(getComputedStyle(el).paddingBottom) || 0;
    const hold = holds.get(el);
    const px = heldBandPx({
      paddingPx,
      scrollTop: el.scrollTop,
      scrollHeight: el.scrollHeight,
      clientHeight: el.clientHeight,
      bandPx,
      heldPx: hold?.px,
      anchorContentPx: hold?.anchorContentPx,
    });
    if (px === 0 || !el.isConnected) { releaseHold(el); return; }
    if (hold?.px === px) return;
    if (hold) hold.px = px;
    else {
      const recheck = () => updateHold(el, lastSetBand);
      // Content that shrinks under the hold may move no scrollTop, so no
      // scroll event says so. A mutation is the other way it shows up.
      const observer = new MutationObserver(recheck);
      observer.observe(el, { childList: true, subtree: true, characterData: true, attributes: true });
      holds.set(el, { px, anchorContentPx: el.scrollHeight - paddingPx, recheck, observer });
      el.addEventListener('scroll', recheck, { passive: true });
    }
    el.style.setProperty('--keyboard-band-hold', `${px}px`);
  };
  // A lower band checks every padded scroller; a higher one only needs to
  // retire the holds it now covers. Both re-check the held ones, which is
  // what retires a hold whose scroller has left the page.
  const holdScrollAnchors = (px: number) => {
    const scrollers = new Set(holds.keys());
    if (px < lastSetBand) {
      document.querySelectorAll<HTMLElement>('.mobile-swipe-pane .content-pane-body')
        .forEach((el) => scrollers.add(el));
    }
    for (const el of scrollers) updateHold(el, px);
  };
  const setBand = (px: number) => {
    if (px === lastSetBand) return;
    holdScrollAnchors(px);
    lastSetBand = px;
    document.documentElement.style.setProperty('--keyboard-band', `${px}px`);
  };
  const syncBand = () => setBand(keyboardBandPx(keyboardReading()));
  const syncShell = () => {
    setHeight(currentAppHeight());
    syncBand();
  };
  /** Reserve the slack now, on the best figure available. A blind viewport
   *  sends no resize when the keys rise, so the focus moves the shell too. */
  const armBand = (e: FocusEvent) => {
    if (!opensSoftwareKeyboard(e.target)) return;
    if (viewportBlind) syncShell();
    else setBand(bandMemory.reserve(layoutViewport()));
  };

  /** The last height ANY observer here acted on. See the poll below, whose
   *  whole job is the change nothing else saw. */
  let observedHeight = vv.height;

  const onResize = () => {
    observedHeight = vv.height;
    // The keyboard opening or closing changes --app-height, which shortens or
    // lengthens the transcript's viewport. The reader is left exactly where
    // they are: the content above them has not moved, so the same scrollTop
    // still shows them the same thing. (This used to re-pin a bottom reader
    // to the new bottom; see scrollState's header for why nothing does that
    // any more.)
    setHeight(currentAppHeight());
    syncBand();
    // A keyboard close leaves WKWebView routing touches against the
    // keyboard-up geometry, and the page cannot read that. Relayout frees it.
    // Called last, so the bounce starts from the height just written.
    noteViewportResize({ height: vv.height, layoutViewport: window.innerHeight });
  };
  // A rotation, or iOS correcting `innerHeight` with only a window resize. A
  // portrait wake after a landscape suspend reports the landscape height at
  // first, and no visual-viewport resize follows the fix (ADR 0610).
  const onLayoutViewportChange = () => {
    observedHeight = vv.height;
    setHeight(currentAppHeight());
    syncBand();
  };
  // A window resize that moved nothing since the last write is a re-announce.
  // Inside a focus window, before the keys rise, acting on it would run
  // `syncBand` on a full-height reading and wipe the reserve `armBand` made.
  const onWindowResize = () => {
    if (layoutViewportKey() !== measuredLayoutViewport) onLayoutViewportChange();
  };
  // The keyboard leaving is a focus change, and it may fire no resize at all
  // when a hardware keyboard was in play. Recompute from live metrics so the
  // slack cannot outlive the keys.
  //
  // A move between two fields is NOT that. The keys stay up, and dropping the
  // padding for even one frame shortens the scroll range: a container near
  // its end is clamped, and the content jumps by a keyboard's height. So the
  // handoff is left alone, as `useHideOnScroll` leaves its own.
  const onFocusOut = (e: FocusEvent) => {
    if (opensSoftwareKeyboard(e.relatedTarget)) return;
    if (viewportBlind) syncShell();
    else syncBand();
  };
  // An iOS resume often drops the keyboard and fires no visual-viewport
  // resize. Without this recompute the shell keeps its keyboard-up height.
  //
  // Two things go wrong at a wake, and both need handling:
  //
  //   1. `vv.height` is often pinned at the stale shrunk value. So the wake
  //      writes through `computeAppHeight`, never a raw `vv.height`.
  //   2. iOS keeps focus on the textarea across the suspend, with the keys
  //      down. That ghost focus would make a late resize re-stamp the shrunk
  //      height, so the wake blurs it. The user taps again to type.
  //
  // The blur runs only when `vv.height` looks shrunk, or gives no reading
  // (iOS 27 reports 0). It also runs while the viewport is blind, which
  // cannot say the keys went down. A quick return to a focused search box
  // keeps focus.
  //
  // Resetting lastSetHeight forces the write even when the value is cached.
  const onWake = () => {
    const vvLooksShrunk =
      !readableVisualViewport() || viewportIsKeyboardShrunk(vv.height, window.innerHeight);
    const active = document.activeElement;
    if ((vvLooksShrunk || viewportBlind) && active instanceof HTMLElement && opensSoftwareKeyboard(active)) {
      active.blur();
    }
    lastSetHeight = -1;
    observedHeight = vv.height;
    setHeight(currentAppHeight());
    syncBand();
    // A resume is a keyboard close the fold would otherwise never see: the
    // comment above says iOS fires no resize here. Told after the height is
    // written, so the bounce starts from the restored one, as `onResize`
    // does.
    //
    // Which half depends on whether iOS left the height PINNED. A pinned one
    // offers the fold no edge, so the stamp goes on this handler's word. A
    // corrected one already carries the restored reading the fold wants.
    // Handing it over is the only way that close is ever seen.
    if (vvLooksShrunk) noteKeyboardClosed();
    else noteWakeViewport({ height: vv.height, layoutViewport: window.innerHeight });
  };
  const onVisibilityChange = () => {
    if (document.visibilityState === 'visible') onWake();
  };
  // The backstop for a close iOS announces to nobody. It stands in for the
  // resize that never came, so it runs `onResize`'s own three steps in the
  // same order.
  //
  // It lives HERE because those steps need the height owner. A poll parked
  // elsewhere would bounce off the keyboard-shrunk --app-height and restore
  // it. The shell would stay at half height, with the black band `onWake`
  // documents above. This effect is mobile-only, so no desktop pays for it.
  //
  // Behind the perf gate (utils/perfQueue.ts), which owns every timer a
  // diagnostic costs. Read live, so the toggle needs no reload. It goes with
  // the dead-press probe (docs/temporary-measures.md), since ADR 0262 found
  // the close it watches for was never the cause.
  let pollTimer: ReturnType<typeof setInterval> | null = null;
  const stopPoll = () => {
    if (pollTimer === null) return;
    clearInterval(pollTimer);
    pollTimer = null;
  };
  const pollViewport = () => {
    if (document.visibilityState === 'hidden') return;
    // The gate can drop with nothing announcing it, so an active tick reads
    // it. Without this the interval outlives what armed it.
    if (!perfRecordingOn()) { stopPoll(); return; }
    // The change NOTHING else saw, which is the silent close this exists for.
    // A handled one already moved `observedHeight`. Acting on it again would
    // run `syncBand` with the keys down inside a live focus window, wiping
    // the reserve `armBand` just made.
    if (vv.height === observedHeight) return;
    observedHeight = vv.height;
    setHeight(currentAppHeight());
    syncBand();
    notePolledViewport({ height: vv.height, layoutViewport: window.innerHeight });
  };
  const startPoll = () => {
    if (pollTimer === null) pollTimer = setInterval(pollViewport, KEYBOARD_POLL_MS);
  };
  const unsubscribeGate = onPerfEnabledChange((on) => (on ? startPoll() : stopPoll()));
  if (perfRecordingOn()) startPoll();
  vv.addEventListener('resize', onResize);
  window.addEventListener('orientationchange', onLayoutViewportChange);
  window.addEventListener('resize', onWindowResize);
  document.addEventListener('visibilitychange', onVisibilityChange);
  window.addEventListener('pageshow', onWake);
  document.addEventListener('focusin', armBand, { passive: true });
  document.addEventListener('focusout', onFocusOut, { passive: true });
  setHeight(currentAppHeight());
  syncBand();
  return () => {
    stopPoll();
    unsubscribeGate();
    vv.removeEventListener('resize', onResize);
    window.removeEventListener('orientationchange', onLayoutViewportChange);
    window.removeEventListener('resize', onWindowResize);
    document.removeEventListener('visibilitychange', onVisibilityChange);
    window.removeEventListener('pageshow', onWake);
    document.removeEventListener('focusin', armBand);
    document.removeEventListener('focusout', onFocusOut);
    for (const el of Array.from(holds.keys())) releaseHold(el);
    document.documentElement.style.removeProperty('--app-height');
    document.documentElement.style.removeProperty('--keyboard-band');
  };
}

/** Mobile-only swipeable container with three full-screen views.
 *
 *  Architecture: CSS transform (not scroll) — mobileView signal is the
 *  single source of truth for both pane position AND header display.
 *  The header reads mobileView to show/hide sections via CSS.
 *  The track reads mobileView to position panes via useLayoutEffect
 *  (runs BEFORE paint — no frame where header and pane disagree).
 *
 *  Desync prevention:
 *  1. All pane navigation goes through navigateToPane() which atomically
 *     closes drawers + updates the signal.
 *  2. useLayoutEffect (not useEffect) derives CSS transform from signal
 *     before the browser paints — header/dots and pane move in the same frame.
 *  3. transitionend handler reconciles transform as a safety net.
 *  4. PaneDrag decides who leads the strip (paneDrag.ts); `stripTrack` is
 *     the only writer of its transform. */
export function MobileSwipeContainer() {
  const containerRef = useRef<HTMLDivElement>(null);
  const trackRef = useRef<HTMLDivElement>(null);
  const mountedRef = useRef(false);
  const dragRef = useRef<PaneDrag | null>(null);
  if (dragRef.current === null) {
    dragRef.current = new PaneDrag(stripTrack(containerRef, trackRef, () => dragRef.current?.holding ?? false), {
      // A frame has already locked its drag horizontal, so the host's only
      // gates are its own. Only a frame inside the strip may lead it, which
      // keeps out any frame laid over or beside the panes.
      frameMayLead: (source) => shouldStartPaneSwipe({
        textInputFocused: opensSoftwareKeyboard(document.activeElement),
        targetScrollable: false,
        appFullscreen: appIsFullscreen(),
      }) && isFrameInside(containerRef.current, source),
      mayCommit: () => !appIsFullscreen(),
    });
  }
  const drag = dragRef.current;

  // The other half of the `--app-height` write below. Shrinking the shell for
  // the keyboard is what puts a focused field behind it. So the reveal that
  // scrolls it back mounts with the shell that shrinks.
  useFocusedFieldVisible();

  // Must be useLayoutEffect — see component JSDoc. A navigation from anywhere
  // ends a drag, and the strip slides to the new pane. The first render snaps.
  useLayoutEffect(() => {
    drag.interrupt(mountedRef.current);
    mountedRef.current = true;
    // The pane half of the navigation mark. This effect already runs on exactly
    // the transition being measured, before paint, so the rAF lands on the frame
    // the user sees. Same shape as `thread-render` in ThreadView.
    reportNavigation('pane');
  }, [mobileView.value]);

  // Safety net: after every CSS transition on the track ends, verify the
  // transform matches the mobileView signal. If something caused them to
  // disagree (interrupted transition, browser quirk), this corrects it without
  // a visible jump. A held strip is the drag's to write.
  useEffect(() => {
    const track = trackRef.current;
    const container = containerRef.current;
    if (!track || !container) return;
    const onTransitionEnd = (e: TransitionEvent) => {
      if (e.target !== track || e.propertyName !== 'transform' || drag.holding) return;
      const paneWidth = container.offsetWidth;
      if (paneWidth <= 0) return;
      const correctValue = paneTransform(PANE_INDEX[mobileView.value], paneWidth);
      if (track.style.transform !== correctValue) {
        track.style.transform = correctValue;
      }
    };
    track.addEventListener('transitionend', onTransitionEnd);
    return () => track.removeEventListener('transitionend', onTransitionEnd);
  }, []);

  // Handle resize: snap to current pane without animation.
  // Guards against height-only changes (e.g., iOS keyboard) to avoid
  // unnecessary transition disable/re-enable on every keyboard toggle.
  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;
    let lastPaneWidth = 0;
    const observer = new ResizeObserver(() => {
      const paneWidth = container.offsetWidth;
      if (paneWidth === 0 || paneWidth === lastPaneWidth) return;
      lastPaneWidth = paneWidth;
      drag.interrupt(false);
    });
    observer.observe(container);
    return () => observer.disconnect();
  }, []);

  // Entering or leaving pseudo-fullscreen ends a drag. Leaving snaps: the CSS
  // override (transform: none !important) drops, which would otherwise
  // animate the track back to its pane position.
  const wasPseudoFullscreen = useRef(appPseudoFullscreen.value);
  useLayoutEffect(() => {
    if (wasPseudoFullscreen.current !== appPseudoFullscreen.value) drag.interrupt(false);
    wasPseudoFullscreen.current = appPseudoFullscreen.value;
  }, [appPseudoFullscreen.value]);

  // Host touches on the strip. A row re-rendered mid-drag takes the rest of
  // its touch with it, so the drag follows the touch onto that row too.
  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    return attachPaneDragTouches(el, drag, { gate: gateStripTouch });
  }, []);

  // A drag inside an app frame or the HTML artifact preview. A frame captures
  // its touches, so code inside it tracks them and posts the drag up
  // (`frame-swipe-bridge.ts`). A frame whose document goes away announces it,
  // since its release never comes.
  useEffect(() => subscribeFrameSwipes((source, message) => {
    if (message.kind === 'drag') drag.frameDrag(source, message.dx);
    else if (message.kind === 'release') drag.frameRelease(source, message.paneDelta);
    else drag.frameGone(source);
  }), []);

  // ── Keyboard-active: block accidental taps on other elements ─────────

  // When a textarea is focused on mobile, set data-keyboard-active on
  // <html> so CSS can disable pointer-events on non-textarea elements.
  useEffect(() => {
    const root = document.documentElement;
    let fieldFocusedAt = 0;
    let hardwareKeyPressed = false;
    const keyboardShrunk = (vv: VisualViewport) =>
      viewportIsKeyboardShrunk(vv.height, window.innerHeight);
    const onFocusIn = (e: FocusEvent) => {
      if (opensSoftwareKeyboard(e.target)) {
        fieldFocusedAt = performance.now();
        hardwareKeyPressed = false;
      }
      if (isKeyboardActiveTarget(e.target)) root.setAttribute('data-keyboard-active', '');
    };
    const onKeyDown = () => {
      const vv = window.visualViewport;
      if (vv && !keyboardShrunk(vv)) hardwareKeyPressed = true;
    };
    // A touch anywhere but the field ends ghost focus, so the same touch can
    // start a pane swipe. A touch on the field is the user opening the keys.
    const releaseGhostFocus = (e: TouchEvent) => {
      const active = document.activeElement;
      const vv = window.visualViewport;
      if (!vv || !(active instanceof HTMLElement) || active.contains(e.target as Node)) return;
      if (isGhostFocus({
        fieldFocused: opensSoftwareKeyboard(active),
        keyboardShrunk: keyboardShrunk(vv),
        focusHeldMs: performance.now() - fieldFocusedAt,
        hardwareKeyPressed,
      })) active.blur();
    };
    const onFocusOut = (e: FocusEvent) => {
      // relatedTarget is the next focused element — if it's also a triggering
      // textarea, keep the attribute (user tapped between prompt fields).
      if (!isKeyboardActiveTarget(e.relatedTarget)) root.removeAttribute('data-keyboard-active');
    };
    // Sweep the flag back into agreement with the live focus at the moments a
    // stale one would first be FELT: the user's next touch (capture phase, so an
    // inert target still reports it) and a return to the foreground. See
    // `reconcileKeyboardActive` for the transition that fires no event at all.
    const reconcile = () => reconcileKeyboardActive(root, document.activeElement);
    const onTouchStart = (e: TouchEvent) => {
      releaseGhostFocus(e);
      reconcile();
    };
    document.addEventListener('focusin', onFocusIn, { passive: true });
    document.addEventListener('focusout', onFocusOut, { passive: true });
    document.addEventListener('keydown', onKeyDown, { passive: true, capture: true });
    document.addEventListener('touchstart', onTouchStart, { passive: true, capture: true });
    document.addEventListener('visibilitychange', reconcile, { passive: true });
    window.addEventListener('pageshow', reconcile, { passive: true });
    return () => {
      document.removeEventListener('focusin', onFocusIn);
      document.removeEventListener('focusout', onFocusOut);
      document.removeEventListener('keydown', onKeyDown, { capture: true });
      document.removeEventListener('touchstart', onTouchStart, { capture: true });
      document.removeEventListener('visibilitychange', reconcile);
      window.removeEventListener('pageshow', reconcile);
      root.removeAttribute('data-keyboard-active');
    };
  }, []);

  // ── iOS Safari workarounds ─────────────────────────────────────────────

  // iOS Safari can scroll the document (window.scrollY > 0) when the
  // keyboard opens or during certain touch interactions.
  useEffect(() => {
    const onWindowScroll = () => {
      if (window.scrollY !== 0) window.scrollTo(0, 0);
    };
    window.addEventListener('scroll', onWindowScroll, { passive: true });
    return () => window.removeEventListener('scroll', onWindowScroll);
  }, []);

  // The strip is `overflow: hidden` (ADR 0420), so a focus() or a scroll into
  // view can slide every pane off its transform. Put it back on the next scroll
  // event.
  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;
    const onContainerScroll = () => {
      if (container.scrollLeft !== 0) container.scrollLeft = 0;
    };
    container.addEventListener('scroll', onContainerScroll, { passive: true });
    return () => container.removeEventListener('scroll', onContainerScroll);
  }, []);

  // Track visual viewport for iOS keyboard handling (--app-height).
  useEffect(trackAppHeight, []);

  return (
    <div class="mobile-swipe-wrapper">
      <div ref={containerRef} class="mobile-swipe-container">
        <div ref={trackRef} class="mobile-swipe-track">
          {MOBILE_VIEWS.map((v) => {
            const { Pane } = MOBILE_PANE_CONFIGS[v];
            return (
              <div key={v} class="mobile-swipe-pane" data-offscreen={isOffscreenPane(v, mobileView.value) ? '' : undefined}>
                <Pane />
                {v === 'thread' && <DashboardPanel />}
                {/* Rendered inside each pane (not the swipe container) so they
                    share a stacking context with the prompt-area, allowing
                    .prompt-area's z-index:2 to keep its buttons clickable. */}
                <EdgeSwipeZones />
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}
