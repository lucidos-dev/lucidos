import { cancelledPressWasATap } from '@lucidos/tap-press';
import { opensSoftwareKeyboard } from '../../utils/dom';

/** The part of an element the replay target decision reads. Structural, so the
 *  decision unit-tests without a DOM. */
export interface ReplayCandidate {
  tagName: string;
  classList: { contains(name: string): boolean };
}

/** Pure decision: where does a tap that began on an edge strip land? The
 *  topmost element of the stack that is not a strip. `null` for an app frame,
 *  because a host cannot click into one: the SDK's own touch bridge owns it. */
export function pickReplayTarget<T extends ReplayCandidate>(stack: readonly T[]): T | null {
  const top = stack.find((el) => !el.classList.contains('edge-swipe-zone'));
  return top && top.tagName !== 'IFRAME' ? top : null;
}

interface PressContact {
  identifier: number;
  screenX: number;
  screenY: number;
  clientX: number;
  clientY: number;
}

interface PressEventLike {
  changedTouches: ArrayLike<PressContact>;
  touches: ArrayLike<unknown>;
}

/** What the replay needs off the document. */
export interface ReplayHost {
  addEventListener(type: string, listener: (e: never) => void, capture: boolean): void;
  removeEventListener(type: string, listener: (e: never) => void, capture: boolean): void;
  elementsFromPoint(x: number, y: number): readonly ReplayCandidate[];
  now(): number;
  dispatchClick(target: ReplayCandidate, x: number, y: number): void;
}

/** Hand a click to the row under an edge strip.
 *
 *  The strip sits over the content, and the gate cancels the touchstart that
 *  lands on it. WebKit then drops the synthetic click, so a tap on a row near
 *  the edge does nothing.
 *
 *  Call this for a touch the gate cancelled. It watches that touch. A lift
 *  that is a tap clicks the element under the finger. A tap is one finger,
 *  within the tap threshold, and not held to a long press.
 *
 *  The lift is read at `document` capture, since a re-render can strand the
 *  strip's own touch. Returns the stop, which ends the watch early. */
export function replayEdgeTap(host: ReplayHost, start: PressEventLike): () => void {
  const first = start.changedTouches[start.changedTouches.length - 1];
  if (!first) return () => {};
  const startedAt = host.now();
  let fingers = start.touches.length;
  let movedPx = 0;

  const contactOf = (e: PressEventLike): PressContact | null => {
    for (let i = 0; i < e.changedTouches.length; i++) {
      if (e.changedTouches[i].identifier === first.identifier) return e.changedTouches[i];
    }
    return null;
  };
  const travel = (e: PressEventLike) => {
    const c = contactOf(e);
    if (!c) return;
    movedPx = Math.max(movedPx, Math.abs(c.screenX - first.screenX), Math.abs(c.screenY - first.screenY));
  };
  const onStart = (e: PressEventLike) => {
    fingers = Math.max(fingers, e.touches.length);
  };
  const onMove = (e: PressEventLike) => {
    onStart(e);
    travel(e);
  };
  const onEnd = (e: PressEventLike) => {
    const c = contactOf(e);
    if (!c) return;
    stop();
    travel(e);
    if (!cancelledPressWasATap({
      heldMs: host.now() - startedAt,
      movedPx,
      fingers,
      fingersLeft: e.touches.length,
    })) return;
    const target = pickReplayTarget(host.elementsFromPoint(c.clientX, c.clientY));
    if (target) host.dispatchClick(target, c.clientX, c.clientY);
  };
  const onCancel = (e: PressEventLike) => {
    if (contactOf(e)) stop();
  };

  const listeners: Array<[string, (e: never) => void]> = [
    ['touchstart', onStart],
    ['touchmove', onMove],
    ['touchend', onEnd],
    ['touchcancel', onCancel],
  ];
  function stop() {
    for (const [type, fn] of listeners) host.removeEventListener(type, fn, true);
  }
  for (const [type, fn] of listeners) host.addEventListener(type, fn, true);
  return stop;
}

/** The real document as a `ReplayHost`. */
export function documentReplayHost(doc: Document = document): ReplayHost {
  return {
    addEventListener: (type, listener, capture) => doc.addEventListener(type, listener as EventListener, capture),
    removeEventListener: (type, listener, capture) => doc.removeEventListener(type, listener as EventListener, capture),
    elementsFromPoint: (x, y) => doc.elementsFromPoint(x, y),
    now: () => Date.now(),
    dispatchClick: (target, x, y) => {
      // A synthetic click never focuses, so a text field takes focus here.
      // The touchend is a user gesture, so iOS still opens the keyboard.
      if (opensSoftwareKeyboard(target as Element)) (target as HTMLElement).focus({ preventScroll: true });
      (target as Element).dispatchEvent(new MouseEvent('click', {
        bubbles: true,
        cancelable: true,
        composed: true,
        clientX: x,
        clientY: y,
      }));
    },
  };
}
