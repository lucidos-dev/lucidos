/** Geometry for the mobile transcript's own scroll indicator.
 *
 *  WHY mobile draws one: the app header is `position: fixed`, and the header
 *  space is a `::before` spacer INSIDE the scroller (styles/mobile.css). Content
 *  has to scroll up under the chrome for hide-on-scroll to reclaim that space.
 *  So a native overlay indicator spans the chrome covering the scroller's top,
 *  and nothing in CSS insets it. We suppress it and draw our own over the region
 *  the content occupies.
 *
 *  It measures the DRAWN slice, as a native scrollbar does (ADR 0258). The
 *  transcript draws a window of a paged thread, and no estimate of the undrawn
 *  history is right on every thread. So the thumb jumps when older turns draw.
 *
 *  Pure and DOM-free, so unit tests over these functions are the gate. */

/** Smallest thumb we will draw, in px. Below this a thumb on a very long thread
 *  becomes a dot that is hard to see and hard to read a position from. Matches
 *  what native overlay indicators do at their floor. */
export const MIN_THUMB_PX = 24;

/** Largest share of the track the thumb may take, as a fraction.
 *
 *  Sized purely by proportion, a thread only just past one screen gives a thumb
 *  filling almost the whole track, which reads as a slab rather than a position
 *  marker and leaves it barely any travel to show the position WITH. The cap
 *  costs little where it bites: it applies below `1 / MAX_THUMB_FRACTION`
 *  screens of content, and the shorter the thread the less interesting "how much
 *  of it is on screen" was going to be. Past that the thumb is proportional
 *  again, so this only ever trades away precision at the short end.
 *
 *  A quarter rather than a half: half a track still reads as a bar rather than a
 *  marker. Clamping a wider band of lengths to one size also steadies the thumb
 *  while the render window grows. */
export const MAX_THUMB_FRACTION = 0.25;

/** What a single scroll event says about why the scroller moved. */
export interface ScrollEventContext {
  /** `isUserScrolling()` (utils/scrollActivity.ts): a touch drag, or its
   *  momentum tail, happened within the last `USER_SCROLL_WINDOW_MS`. */
  userScrolling: boolean;
  /** `isNavigationScroll()` (components/chat/scrollState.ts): this event
   *  came from one of our own navigations writing scrollTop frame by frame (a
   *  chevron tap, turn-nav, a deep-link glide), not from the reader. */
  programmaticScroll: boolean;
  /** `isRepaintNudging()` (utils/webkitRepaint.ts): the compositor-recovery nudge
   *  writes +/-1px and puts it back a frame later, firing two real scroll events
   *  for a movement the user never made. */
  repaintNudge: boolean;
}

/** Whether the indicator is up, and whether to restart its fade-out timer. */
export interface IndicatorVisibility {
  shown: boolean;
  /** Restart the hide countdown. False leaves an already-running one to expire,
   *  which is how the indicator fades.
   *
   *  INVARIANT: a result that newly turns `shown` on always arms. The countdown
   *  is the ONLY thing that ever turns the indicator off, so a summon that does
   *  not start one leaves it lit until some later event happens to arm one, and
   *  if the scroller goes quiet first it stays lit forever. Pinned exhaustively
   *  in the geometry suite. */
  armHideTimer: boolean;
}

/**
 * Whether the indicator should be up after this scroll event.
 *
 * SUMMONING and STAYING LIT are deliberately driven by different signals, and
 * conflating them is a bug this function exists to prevent:
 *
 *  - **Summon on intent.** Only a touch drag brings the indicator up. An
 *    event-summoned indicator would sit on screen for any scroll the app made
 *    on its own, while the user was doing nothing.
 *  - **Stay lit on motion.** Once summoned, ANY real movement keeps it up, touch
 *    or not. The first version restarted the timer only while `userScrolling`
 *    was true, i.e. for 1200ms after the last `touchmove` -- so a hard flick
 *    whose iOS momentum ran longer than that had its indicator fade out while
 *    the content was still visibly scrolling ("the indicator sometimes
 *    disappears during scroll"). Momentum is not touch, but it IS the user's
 *    scroll.
 *
 * Two kinds of movement are excluded from "real", because neither is the user
 * scrolling and both would otherwise hold a once-summoned indicator up on a live
 * thread: a *navigation scroll* (the app taking the reader somewhere they asked
 * to go, e.g. a chevron tap or a deep-link glide), and the iOS
 * compositor-recovery nudge, which fires scroll events on a ~200ms throttle for
 * a movement of one pixel that is immediately undone.
 *
 * Both exclusions are subordinate to the user, though: a drag that overlaps
 * either one still counts as the user scrolling. Letting an exclusion win over a
 * live drag is how the indicator gets STUCK rather than merely dropped, because
 * it can summon the indicator without ever starting the countdown that turns it
 * off again (see the `armHideTimer` invariant).
 */
export function nextIndicatorVisibility(
  shown: boolean,
  ev: ScrollEventContext,
): IndicatorVisibility {
  // A nudge is only ignorable while the user is NOT dragging, or it would eat
  // the user's own scroll events. Same dual gate as hooks/useHideOnScroll.ts,
  // and safe for the same reason: `isUserScrolling()` keys off `touchmove`,
  // which is never produced programmatically, so the nudge cannot trip the
  // bypass itself. Returning without arming cannot strand a lit indicator: a
  // `shown` one always has a countdown already running, by the invariant.
  if (ev.repaintNudge && !ev.userScrolling) return { shown, armHideTimer: false };

  // Same subordination for a navigation scroll: it only suppresses the countdown
  // when the user is not also dragging. Without the `!ev.userScrolling` clause, a
  // finger landing on the transcript mid-glide summoned the indicator and armed
  // nothing, so it stayed on screen for good once the scroller went quiet.
  const suppressed = ev.programmaticScroll && !ev.userScrolling;
  const nextShown = shown || ev.userScrolling;
  return { shown: nextShown, armHideTimer: nextShown && !suppressed };
}

/**
 * The VERTICAL corner radius to specify so the thumb's caps still render as
 * semicircles after `scaleY` has been applied.
 *
 * The thumb is sized by `transform: scaleY(k)`, and a transform scales the
 * painted corner radius with everything else: a radius authored as `r` renders
 * with a horizontal extent of `r` but a vertical extent of `r * k`. Any `k` away
 * from 1 therefore turns the round caps into ellipses, and since the corners on
 * one edge meet in the middle of a bar this thin, a stretched cap reads as a
 * POINT. That is the "pointy edges" this undoes.
 *
 * Dividing by `k` pre-compensates exactly: `(r / k) * k === r`, so the cap is a
 * true semicircle at every thumb length. Cheap to apply, and `border-radius` is
 * a paint property, so writing it per frame keeps the scroll path free of layout
 * writes (see hooks/useThreadScrollIndicator.ts).
 *
 * Returns `halfWidthPx` unchanged for a non-finite or non-positive scale, which
 * is the undistorted answer and the right fallback for a thumb that is not being
 * drawn anyway.
 */
export function counterScaledRadiusPx(halfWidthPx: number, scaleY: number): number {
  if (!(scaleY > 0) || !Number.isFinite(scaleY)) return halfWidthPx;
  return halfWidthPx / scaleY;
}

/** Scroller metrics, as read from the DOM by the caller. */
export interface ScrollIndicatorInput {
  /** `el.scrollTop`, may be negative or past the max during iOS elastic bounce. */
  scrollTop: number;
  /** `el.scrollHeight`. */
  scrollHeight: number;
  /** `el.clientHeight`. */
  clientHeight: number;
  /** Height of the indicator's track, in px. */
  trackHeightPx: number;
}

/** Where to draw the thumb. `visible: false` means draw nothing. */
export interface ScrollIndicatorGeometry {
  visible: boolean;
  /** Thumb height in px, at least `MIN_THUMB_PX` when visible. */
  thumbHeightPx: number;
  /** Thumb's offset from the top of the track, in px. */
  thumbOffsetPx: number;
}

const HIDDEN: ScrollIndicatorGeometry = { visible: false, thumbHeightPx: 0, thumbOffsetPx: 0 };

/**
 * Thumb size and position within the track: the viewport's share of the
 * scroller's content, and its position within it.
 *
 * `scrollTop` is clamped before use: iOS elastic bounce reports values below 0
 * and past the maximum, which would otherwise push the thumb out of the track at
 * both ends.
 */
export function computeScrollIndicator(input: ScrollIndicatorInput): ScrollIndicatorGeometry {
  const { scrollHeight, clientHeight, trackHeightPx } = input;
  if (!(trackHeightPx > 0) || !(clientHeight > 0) || !(scrollHeight > clientHeight)) return HIDDEN;

  const maxScroll = scrollHeight - clientHeight;
  const scrollTop = Math.min(Math.max(0, input.scrollTop), maxScroll);

  // Floored so a very long thread still shows something readable, and ceilinged
  // so a short one does not (see MAX_THUMB_FRACTION). The ceiling is itself held
  // to the track, which is what keeps a pane too short to fit the floor from
  // producing a thumb taller than the track it sits in.
  const rawThumb = (clientHeight / scrollHeight) * trackHeightPx;
  const ceiling = Math.min(trackHeightPx, Math.max(MIN_THUMB_PX, trackHeightPx * MAX_THUMB_FRACTION));
  const thumbHeightPx = Math.min(ceiling, Math.max(MIN_THUMB_PX, rawThumb));

  // Position over the travel that is actually left once the floored thumb has
  // taken its share, so the thumb still lands flush with both ends of the track.
  // maxScroll > 0 holds because scrollHeight > clientHeight above.
  const thumbOffsetPx = (scrollTop / maxScroll) * (trackHeightPx - thumbHeightPx);

  return { visible: true, thumbHeightPx, thumbOffsetPx };
}
