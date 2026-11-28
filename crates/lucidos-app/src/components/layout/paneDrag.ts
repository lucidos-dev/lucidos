
import { SwipeTouch, followStrandedTouch } from '@lucidos/pane-swipe';
import type { FrameSource } from '../../store/actions/frame-swipe-bridge';

export interface PaneTrack {
  paneWidth(): number;
  grab(): void;
  follow(dx: number): void;
  navigate(paneDelta: -1 | 1): void;
  rest(animate: boolean): void;
}

export interface PaneDragEnv {
  frameMayLead(source: FrameSource): boolean;
  mayCommit(): boolean;
}

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
  private readonly ignoredFrames = new Set<FrameSource>();

  constructor(private readonly track: PaneTrack, private readonly env: PaneDragEnv) {}

  get holding(): boolean {
    return this.lead !== null && (this.lead.kind === 'frame' || this.lead.holding);
  }

  get leadTouch(): number | null {
    return this.lead?.kind === 'touch' ? this.lead.id : null;
  }

  touchStart(touch: TouchPoint, listed: ArrayLike<TouchPoint>, accepted: boolean): boolean {
    this.dropUnlisted(listed);
    if (this.holding && this.lead?.kind === 'touch') return false;
    this.abandon();
    if (!accepted) return false;
    const swipe = new SwipeTouch();
    swipe.start(touch.clientX, touch.clientY);
    this.lead = { kind: 'touch', id: touch.identifier, swipe, holding: false };
    return true;
  }

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

  touchCancel(changed: ArrayLike<TouchPoint>, listed: ArrayLike<TouchPoint>): void {
    const lead = this.lead;
    if (lead?.kind !== 'touch') return;
    if (find(changed, lead.id)) this.abandon();
    else this.dropUnlisted(listed);
  }

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
    this.lead = { kind: 'frame', source };
    this.track.grab();
    this.track.follow(dx);
  }

  frameRelease(source: FrameSource, paneDelta: -1 | 0 | 1): void {
    if (this.ignoredFrames.delete(source)) return;
    const lead = this.lead;
    if (lead?.kind !== 'frame' || lead.source !== source) return;
    this.lead = null;
    this.settle(paneDelta);
  }

  frameGone(source: FrameSource): void {
    this.ignoredFrames.delete(source);
    const lead = this.lead;
    if (lead?.kind !== 'frame' || lead.source !== source) return;
    this.lead = null;
    this.track.rest(true);
  }

  interrupt(animate: boolean): void {
    const lead = this.lead;
    this.lead = null;
    if (lead?.kind === 'frame') this.ignoredFrames.add(lead.source);
    this.track.rest(animate);
  }

  private dropUnlisted(listed: ArrayLike<TouchPoint>): void {
    const lead = this.lead;
    if (lead?.kind === 'touch' && !find(listed, lead.id)) this.abandon();
  }

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
  gate: (e: TouchEvent, touch: TouchPoint) => boolean;
}

function touchLists(e: Event): TouchEvent | null {
  const t = e as TouchEvent;
  return t.touches && t.changedTouches ? t : null;
}

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

interface FrameHost {
  querySelectorAll(selector: 'iframe'): ArrayLike<{ contentWindow: unknown }>;
}

export function isFrameInside(root: FrameHost | null, source: FrameSource): boolean {
  if (!root || !source) return false;
  const frames = root.querySelectorAll('iframe');
  for (let i = 0; i < frames.length; i++) if (frames[i].contentWindow === source) return true;
  return false;
}
