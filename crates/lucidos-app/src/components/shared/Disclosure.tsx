/**
 * A block that unfolds with the *disclosure roll*, the thread drawer's own
 * motion (`docs/glossary.md`). The content slides down from under the row
 * above it and fades in, and everything below moves with it. Closing reverses.
 *
 * The drawer rolls FLIP copies, because its rows live in a fixed-height
 * scroller. A block in normal flow gets the same picture by animating its own
 * height: content anchored to the bottom of a growing clip IS the roll, and
 * layout carries the rows below in lockstep. Both read their curve and length
 * from `utils/disclosureMotion.ts`.
 *
 * Closed content is absent, as it would be under a plain `{open && …}`. It
 * stays only for its exit, inert, as it was last shown open: a caller may stop
 * computing it in the render that closes it. The first render never rolls, so
 * a surface that opens already unfolded simply shows it.
 */
import type { ComponentChildren } from 'preact';
import { useLayoutEffect, useRef, useState } from 'preact/hooks';
import { durationScale, isReducedMotion } from '../../utils/motion';
import { EASING_DISCLOSURE, FADE_REACH, disclosureDurationMs, rollCap } from '../../utils/disclosureMotion';

function canAnimate(instant: boolean): boolean {
  return !instant && !isReducedMotion() && typeof HTMLElement.prototype.animate === 'function';
}

/** A roll nobody can see is a snap. The transcript's two toggles roll every
 *  turn at once, and a long thread would otherwise run hundreds of rolls off
 *  screen. Above the fold a snap also lets the scroll anchor settle at once. */
function offScreen(box: HTMLElement): boolean {
  const { top, bottom } = box.getBoundingClientRect();
  return bottom <= 0 || top >= window.innerHeight;
}

/** Rolls toggled in this commit and not started yet, each as a measure that
 *  returns its start. They run in one microtask once every effect of the
 *  commit is done, and two things depend on that:
 *
 *  - **Each row measures the settled page.** The rows above it have their new
 *    heights, and the scroll anchor has already corrected for them. Measured
 *    from its own effect, a row saw rows above it that were still changing.
 *  - **Every row reads before any row starts.** A start dirties the layout,
 *    so a read after it lays the page out again: once per row, for a
 *    transcript-wide toggle. */
let queuedRolls: (() => () => void)[] | null = null;

function rollAfterCommit(measure: () => () => void) {
  if (!queuedRolls) {
    const rolls: (() => () => void)[] = [];
    queuedRolls = rolls;
    queueMicrotask(() => {
      queuedRolls = null;
      const starts = rolls.map(m => m());
      for (const start of starts) start();
    });
  }
  queuedRolls.push(measure);
}

export function Disclosure({ open, instant = false, children, class: className, bodyClass }: {
  open: boolean;
  /** Toggle at once, with no roll. For a navigation that opens the block to
   *  land on a row inside it: a scroll mid-roll measures a box still moving. */
  instant?: boolean;
  children: ComponentChildren;
  /** Extra classes on the outer box, the one that rolls its height. */
  class?: string;
  /** Extra classes on the box holding the children, for a site whose rows
   *  need their container's layout (a flex gap, say). */
  bodyClass?: string;
}) {
  const outer = useRef<HTMLDivElement>(null);
  const body = useRef<HTMLDivElement>(null);
  const shown = useRef(open);
  const running = useRef<Animation[]>([]);
  // Bumped by every toggle, so a start still queued for an older one drops out.
  const toggles = useRef(0);
  const lastOpenChildren = useRef(children);
  if (open) lastOpenChildren.current = children;
  const [leaving, setLeaving] = useState(false);
  const [rolling, setRolling] = useState(false);
  const [, setRedraws] = useState(0);

  // The render that closes it still draws the content, so the exit has
  // something to roll. `shown` catches up in the effect below.
  const closingNow = shown.current && !open && canAnimate(instant);
  const present = open || leaving || closingNow;

  useLayoutEffect(() => {
    if (shown.current === open) return;
    shown.current = open;
    const toggle = ++toggles.current;
    const box = outer.current;
    const content = body.current;
    const snap = () => {
      // A snap mid-roll drops the roll, or a held exit keeps the box shut.
      for (const a of running.current) a.cancel();
      running.current = [];
      setRolling(false);
      setLeaving(false);
      // The closing render drew the content for an exit that is now a snap.
      if (!open) setRedraws(n => n + 1);
    };
    if (!box || !content || !canAnimate(instant)) {
      snap();
      return;
    }
    setRolling(true);
    setLeaving(!open);

    rollAfterCommit(() => {
      if (toggles.current !== toggle) return () => {};
      // Reads only: the start returned below is the one write.
      const interrupted = running.current.length > 0;
      if (!interrupted && offScreen(box)) return snap;
      const height = content.offsetHeight;
      const rect = box.getBoundingClientRect();
      const from = interrupted ? rect.height : open ? 0 : height;
      const fromOpacity = interrupted ? getComputedStyle(content).opacity : open ? '0' : '1';
      const to = open ? height : 0;
      const roll = rollCap(height, rect.top, window.innerHeight);
      const offset = (h: number) => (height > 0 ? -roll * (1 - h / height) : 0);
      const timing: KeyframeAnimationOptions = {
        duration: disclosureDurationMs(roll) * durationScale.value,
        easing: EASING_DISCLOSURE,
        fill: open ? 'none' : 'forwards',
      };
      // The fade under the reveal line reaches as far as the content has slid
      // under it, so content at rest is never dimmed.
      const frame = (h: number): Keyframe => ({ height: `${h}px`, [FADE_REACH]: `${-offset(h)}px` });

      return () => {
        for (const a of running.current) a.cancel();
        const anims = [
          box.animate([frame(from), frame(to)], timing),
          content.animate([
            { transform: `translateY(${offset(from)}px)`, opacity: fromOpacity },
            { transform: `translateY(${offset(to)}px)`, opacity: open ? '1' : '0' },
          ], timing),
        ];
        running.current = anims;
        Promise.all(anims.map(a => a.finished)).then(() => {
          if (running.current !== anims) return;
          running.current = [];
          setRolling(false);
          setLeaving(false);
        }, () => { /* a newer toggle cancelled it and owns the box now */ });
      };
    });
  }, [open]);

  if (!present) return null;
  const outerClass = ['disclosure', rolling && 'is-rolling', className].filter(Boolean).join(' ');
  const innerClass = ['disclosure-body', bodyClass].filter(Boolean).join(' ');
  return (
    <div ref={outer} class={outerClass}>
      <div ref={body} class={innerClass} inert={!open}>
        {lastOpenChildren.current}
      </div>
    </div>
  );
}
