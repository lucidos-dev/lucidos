/**
 * The theme strip's move when a family chip re-lays it out. Each keyed piece
 * (a card, a family name) slides from where it was to where it lands. A piece
 * that arrives fades in, and one that leaves fades out where it stood, as an
 * inert copy. The strip eases to its new height meanwhile. All of it runs on
 * the disclosure roll's curve (`utils/disclosureMotion.ts`).
 */
import { EASING_DISCLOSURE, disclosureDurationMs } from '../../utils/disclosureMotion';
import { durationScale, isReducedMotion } from '../../utils/motion';

const PIECE = '[data-strip-key]';
const GHOST_CLASS = 'theme-strip-ghost';

/** The strip as it showed just before the chip, mid-move included. */
export interface StripSnapshot {
  strip: DOMRect;
  rects: Map<string, DOMRect>;
  /** The pieces themselves, so one that leaves can still be copied. */
  pieces: Map<string, HTMLElement>;
}

export function snapshotStrip(strip: HTMLElement): StripSnapshot {
  const rects = new Map<string, DOMRect>();
  const pieces = new Map<string, HTMLElement>();
  for (const el of strip.querySelectorAll<HTMLElement>(PIECE)) {
    const key = el.dataset.stripKey!;
    rects.set(key, el.getBoundingClientRect());
    pieces.set(key, el);
  }
  return { strip: strip.getBoundingClientRect(), rects, pieces };
}

function canAnimate(): boolean {
  return !isReducedMotion() && typeof HTMLElement.prototype.animate === 'function';
}

/** Whether a piece shows inside the strip's clip, which scrolls sideways only. */
function inSight(piece: DOMRect, strip: DOMRect): boolean {
  return piece.right > strip.left && piece.left < strip.right;
}

/** A copy of a piece that left, laid over the spot it held. The strip is its
 *  containing block, so the copy scrolls and clips with the cards. It goes in
 *  first, so it paints under the cards sliding across it. */
function ghostOf(piece: HTMLElement, at: DOMRect, strip: HTMLElement, stripRect: DOMRect): HTMLElement {
  const ghost = piece.cloneNode(true) as HTMLElement;
  ghost.removeAttribute('data-strip-key');
  ghost.removeAttribute('role');
  ghost.removeAttribute('aria-checked');
  ghost.setAttribute('aria-hidden', 'true');
  ghost.inert = true;
  ghost.classList.add(GHOST_CLASS);
  ghost.style.gridColumn = '';
  ghost.style.gridRow = '';
  ghost.style.left = `${at.left - stripRect.left - strip.clientLeft + strip.scrollLeft}px`;
  ghost.style.top = `${at.top - stripRect.top - strip.clientTop + strip.scrollTop}px`;
  ghost.style.width = `${at.width}px`;
  ghost.style.height = `${at.height}px`;
  strip.prepend(ghost);
  return ghost;
}

/** Plays the move from `before` to the layout the strip holds now. Returns a
 *  cancel, which drops the move where it is and removes every copy.
 *
 *  Copies and slides widen the strip's scroll width while they run, so
 *  anything read from it mid-move is stale. `onSettled` runs once the move
 *  has finished and every copy is gone, never after a cancel. */
export function playStripMove(strip: HTMLElement, before: StripSnapshot, onSettled: () => void): () => void {
  if (!canAnimate()) return () => {};
  const now = strip.getBoundingClientRect();
  const moves: { el: HTMLElement; dx: number; dy: number }[] = [];
  const arrivals: HTMLElement[] = [];
  const present = new Set<string>();
  for (const el of strip.querySelectorAll<HTMLElement>(PIECE)) {
    const key = el.dataset.stripKey!;
    present.add(key);
    const from = before.rects.get(key);
    const to = el.getBoundingClientRect();
    if (from && inSight(from, before.strip)) {
      const dx = from.left - to.left;
      const dy = from.top - to.top;
      if (Math.abs(dx) >= 0.5 || Math.abs(dy) >= 0.5) moves.push({ el, dx, dy });
    } else if (inSight(to, now)) {
      arrivals.push(el);
    }
  }
  const departures = [...before.pieces].filter(([key]) => !present.has(key) && inSight(before.rects.get(key)!, before.strip));
  const grow = now.height - before.strip.height;
  const distance = Math.max(Math.abs(grow), ...moves.map(m => Math.max(Math.abs(m.dx), Math.abs(m.dy))));
  const timing: KeyframeAnimationOptions = {
    duration: disclosureDurationMs(distance) * durationScale.value,
    easing: EASING_DISCLOSURE,
  };

  const running: Animation[] = [];
  if (Math.abs(grow) >= 0.5) {
    running.push(strip.animate([{ height: `${before.strip.height}px` }, { height: `${now.height}px` }], timing));
  }
  for (const { el, dx, dy } of moves) {
    running.push(el.animate([{ transform: `translate(${dx}px, ${dy}px)` }, { transform: 'none' }], timing));
  }
  for (const el of arrivals) running.push(el.animate([{ opacity: 0 }, { opacity: 1 }], timing));
  const ghosts = departures.map(([key, piece]) => ghostOf(piece, before.rects.get(key)!, strip, now));
  for (const ghost of ghosts) {
    const fade = ghost.animate([{ opacity: 1 }, { opacity: 0 }], { ...timing, fill: 'forwards' });
    running.push(fade);
    fade.finished.then(() => ghost.remove(), () => { /* the cancel below removed it */ });
  }
  // Registered after each copy's removal, so those run first.
  Promise.all(running.map(animation => animation.finished)).then(onSettled, () => { /* cancelled */ });
  return () => {
    for (const animation of running) animation.cancel();
    for (const ghost of ghosts) ghost.remove();
  };
}
