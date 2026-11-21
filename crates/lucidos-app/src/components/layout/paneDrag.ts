/**
 * Who leads the phone's pane strip, and when a drag of it ends.
 *
 * Three sources move the strip: host touches on it, drags an app frame or the
 * HTML preview posts up (`frame-swipe-bridge.ts`), and the header's swipe. The
 * strip follows at most one *lead* at a time: one host touch, by identifier, or
 * one frame. Everything else is ignored until the lead ends.
 *
 * Only a lift commits: the lead touch's `touchend`, or the lead frame's
 * release. Everything else that ends a drag springs back. DOM-free, like
 * `SwipeTouch`, so `paneDrag.test.ts` tests the rules without a browser.
 * See `docs/plans/2026-10-11-a-pane-swipe-reaches-no-invalid-state.md`.
 */

import { SwipeTouch, followStrandedTouch } from '@lucidos/pane-swipe';
import type { FrameSource } from '../../store/actions/frame-swipe-bridge';

/** What a pane drag moves. The strip draws on it, the header's draws nothing. */
export interface PaneTrack {
  /** The width a far drag is measured against, in px. */
  paneWidth(): number;
  /** A drag takes the strip where it is on screen, with the transition off. */
  grab(): void;
  /** The held strip follows a drag `dx` px from where it was grabbed. */
  follow(dx: number): void;
  /** Move to the neighbouring pane, when there is one. */
  navigate(paneDelta: -1 | 1): void;
  /** Rest on the visible pane: slide there, or snap when `animate` is false. */
  rest(animate: boolean): void;
}

export interface PaneDragEnv {
  /** May this frame's gesture lead the strip? Asked once per gesture. */
  frameMayLead(source: FrameSource): boolean;
  /** May a lift change the pane now? A fullscreen app says no. */
  mayCommit(): boolean;
}

/** The part of a `Touch` a drag reads. */
export interface TouchPoint {
  identifier: number;
  clientX: number;
  clientY: number;
}

type Lead =
  | { kind: 'touch'; id: number; swipe: SwipeTouch; holding: boolean }
  | { kind: 'frame'; source: FrameSource };

function find(list: ArrayLike<TouchPoint>, id: number): TouchPoint | null {
  for (let i = 0; i < list.length; i++) if (list[i].identifier === id) return list[i];
  return null;
}

export class PaneDrag {
  private lead: Lead | null = null;
  /** Frames whose current gesture is ignored until its release or `gone`. */
  private readonly ignoredFrames = new Set<FrameSource>();

  constructor(private readonly track: PaneTrack, private readonly env: PaneDragEnv) {}

  /** Whether a drag holds the strip, so nothing else may write it. */
  get holding(): boolean {
    return this.lead !== null && (this.lead.kind === 'frame' || this.lead.holding);
  }

  /** The identifier of the touch this drag follows, if one leads. */
  get leadTouch(): number | null {
    return this.lead?.kind === 'touch' ? this.lead.id : null;
  }

  /** A new touch. `listed` is the event's own `touches`, `accepted` the
   *  caller's gates. Returns whether the touch now leads. */
  touchStart(touch: TouchPoint, listed: ArrayLike<TouchPoint>, accepted: boolean): boolean {
    this.dropUnlisted(listed);
    if (this.holding && this.lead?.kind === 'touch') return false;
    // An unlocked touch gives way to the newest one. A frame drag ends: a host
    // touch means its finger is not in the frame, so its release was lost.
    this.abandon();
    if (!accepted) return false;
    const swipe = new SwipeTouch();
    swipe.start(touch.clientX, touch.clientY);
    this.lead = { kind: 'touch', id: touch.identifier, swipe, holding: false };
    return true;
  }

  /** A touch moved. Returns whether the move drove the strip, so the caller
   *  cancels it. */
  touchMove(listed: ArrayLike<TouchPoint>, cancelable: boolean): boolean {
    const lead = this.lead;
    if (lead?.kind !== 'touch') return false;
    const t = find(listed, lead.id);
    if (!t) {
      this.abandon();
      return false;
    }
    const dx = lead.swipe.move(t.clientX, t.clientY);
    if (dx === null) return false;
    // An uncancelable move means the browser already owns a native pan.
    if (!cancelable) {
      this.abandon();
      return false;
    }
    if (!lead.holding) {
      lead.holding = true;
      this.track.grab();
    }
    this.track.follow(dx);
    return true;
  }

  /** Touches lifted. Only the lead's own lift commits. */
  touchEnd(changed: ArrayLike<TouchPoint>, listed: ArrayLike<TouchPoint>): void {
    const lead = this.lead;
    if (lead?.kind !== 'touch') return;
    if (find(changed, lead.id)) {
      this.lead = null;
      if (lead.holding) this.settle(lead.swipe.end(this.track.paneWidth()));
      return;
    }
    this.dropUnlisted(listed);
  }

  /** Touches the browser took over. The lead's springs back. */
  touchCancel(changed: ArrayLike<TouchPoint>, listed: ArrayLike<TouchPoint>): void {
    const lead = this.lead;
    if (lead?.kind !== 'touch') return;
    if (find(changed, lead.id)) this.abandon();
    else this.dropUnlisted(listed);
  }

  /** A frame's sideways drag, `dx` px from where the frame locked it. */
  frameDrag(source: FrameSource, dx: number): void {
    if (this.ignoredFrames.has(source)) return;
    const lead = this.lead;
    if (lead?.kind === 'frame' && lead.source === source) {
      this.track.follow(dx);
      return;
    }
    if (this.holding || !this.env.frameMayLead(source)) {
      this.ignoredFrames.add(source);
      return;
    }
    // A touch that has not locked sideways gives way, as it does to a newer touch.
    this.lead = { kind: 'frame', source };
    this.track.grab();
    this.track.follow(dx);
  }

  /** A frame's drag ended. Only the lead frame's release commits. */
  frameRelease(source: FrameSource, paneDelta: -1 | 0 | 1): void {
    if (this.ignoredFrames.delete(source)) return;
    const lead = this.lead;
    if (lead?.kind !== 'frame' || lead.source !== source) return;
    this.lead = null;
    this.settle(paneDelta);
  }

  /** A frame's document went away, so its release never comes. */
  frameGone(source: FrameSource): void {
    this.ignoredFrames.delete(source);
    const lead = this.lead;
    if (lead?.kind !== 'frame' || lead.source !== source) return;
    this.lead = null;
    this.track.rest(true);
  }

  /** Something else moved the strip: a navigation, a resize or fullscreen.
   *  The drag ends, and the rest of its gesture does nothing. */
  interrupt(animate: boolean): void {
    const lead = this.lead;
    this.lead = null;
    if (lead?.kind === 'frame') this.ignoredFrames.add(lead.source);
    this.track.rest(animate);
  }

  /** A lead the browser no longer lists ended unseen. */
  private dropUnlisted(listed: ArrayLike<TouchPoint>): void {
    const lead = this.lead;
    if (lead?.kind === 'touch' && !find(listed, lead.id)) this.abandon();
  }

  /** End the lead without a lift: a held strip springs back. A frame stays
   *  ignored until its own release, which the SDK always sends. */
  private abandon(): void {
    const wasHolding = this.holding;
    const lead = this.lead;
    this.lead = null;
    if (lead?.kind === 'frame') this.ignoredFrames.add(lead.source);
    if (wasHolding) this.track.rest(true);
  }

  private settle(paneDelta: number): void {
    if (paneDelta !== 0 && this.env.mayCommit()) this.track.navigate(paneDelta > 0 ? 1 : -1);
    this.track.rest(true);
  }
}

export interface PaneDragTouchOptions {
  /** May a pane swipe start for this new touch? Runs for every new touch, so
   *  a caller may also act on it here, as the strip's edge guard does. */
  gate: (e: TouchEvent, touch: TouchPoint) => boolean;
}

/** The touch lists of a real touch event, or null for an event without them. */
function touchLists(e: Event): TouchEvent | null {
  const t = e as TouchEvent;
  return t.touches && t.changedTouches ? t : null;
}

/** Drive `drag` from the touches on `surface`, by identifier. Synthetic events
 *  count, so a test can swipe. The lead touch is also followed onto its own
 *  node, for when that node leaves the DOM (`followStrandedTouch`). Returns the
 *  cleanup. */
export function attachPaneDragTouches(
  surface: EventTarget,
  drag: PaneDrag,
  { gate }: PaneDragTouchOptions,
): () => void {
  let unfollow = () => {};
  const stopFollowing = () => {
    unfollow();
    unfollow = () => {};
  };
  const stopFollowingUnlessLed = () => {
    if (drag.leadTouch === null) stopFollowing();
  };

  const onStart = (e: Event) => {
    const t = touchLists(e);
    if (!t || t.changedTouches.length === 0) return;
    const touch = t.changedTouches[t.changedTouches.length - 1];
    if (drag.touchStart(touch, t.touches, gate(t, touch))) {
      stopFollowing();
      unfollow = followStrandedTouch(e, surface, { move: onMove, end: onEnd, cancel: onCancel });
    }
    stopFollowingUnlessLed();
  };
  const onMove = (e: Event) => {
    const t = touchLists(e);
    if (!t) return;
    if (drag.touchMove(t.touches, e.cancelable)) e.preventDefault();
    stopFollowingUnlessLed();
  };
  const onEnd = (e: Event) => {
    const t = touchLists(e);
    if (!t) return;
    drag.touchEnd(t.changedTouches, t.touches);
    stopFollowingUnlessLed();
  };
  const onCancel = (e: Event) => {
    const t = touchLists(e);
    if (!t) return;
    drag.touchCancel(t.changedTouches, t.touches);
    stopFollowingUnlessLed();
  };

  // touchstart is not passive: the strip's gate cancels it at the screen edges.
  const listeners: Array<[string, (e: Event) => void, AddEventListenerOptions]> = [
    ['touchstart', onStart, { passive: false }],
    ['touchmove', onMove, { passive: false }],
    ['touchend', onEnd, { passive: true }],
    ['touchcancel', onCancel, { passive: true }],
  ];
  for (const [type, fn, options] of listeners) surface.addEventListener(type, fn, options);
  return () => {
    for (const [type, fn, options] of listeners) surface.removeEventListener(type, fn, options);
    stopFollowing();
  };
}

/** The part of the DOM the containment check reads. */
interface FrameHost {
  querySelectorAll(selector: 'iframe'): ArrayLike<{ contentWindow: unknown }>;
}

/** Whether `source` is the window of a frame inside `root`. Only a frame
 *  inside the pane strip may lead it, so a frame outside it may not. */
export function isFrameInside(root: FrameHost | null, source: FrameSource): boolean {
  if (!root || !source) return false;
  const frames = root.querySelectorAll('iframe');
  for (let i = 0; i < frames.length; i++) if (frames[i].contentWindow === source) return true;
  return false;
}
