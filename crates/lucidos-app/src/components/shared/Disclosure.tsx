import type { ComponentChildren } from 'preact';
import { useLayoutEffect, useRef, useState } from 'preact/hooks';
import { durationScale, isReducedMotion } from '../../utils/motion';
import { EASING_DISCLOSURE, FADE_REACH, disclosureDurationMs, rollCap } from '../../utils/disclosureMotion';

function canAnimate(instant: boolean): boolean {
  return !instant && !isReducedMotion() && typeof HTMLElement.prototype.animate === 'function';
}

function offScreen(box: HTMLElement): boolean {
  const { top, bottom } = box.getBoundingClientRect();
  return bottom <= 0 || top >= window.innerHeight;
}

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

export function Disclosure({ open, instant = false, appear = false, onClosed, children, class: className, bodyClass }: {
  open: boolean;
  instant?: boolean;
  appear?: boolean;
  onClosed?: () => void;
  children: ComponentChildren;
  class?: string;
  bodyClass?: string;
}) {
  const outer = useRef<HTMLDivElement>(null);
  const body = useRef<HTMLDivElement>(null);
  const shown = useRef(appear ? false : open);
  const onClosedRef = useRef(onClosed);
  onClosedRef.current = onClosed;
  const running = useRef<Animation[]>([]);
  const toggles = useRef(0);
  const lastOpenChildren = useRef(children);
  if (open) lastOpenChildren.current = children;
  const [leaving, setLeaving] = useState(false);
  const [rolling, setRolling] = useState(false);
  const [, setRedraws] = useState(0);

  const closingNow = shown.current && !open && canAnimate(instant);
  const present = open || leaving || closingNow;

  useLayoutEffect(() => {
    if (shown.current === open) return;
    shown.current = open;
    const toggle = ++toggles.current;
    const box = outer.current;
    const content = body.current;
    const snap = () => {
      for (const a of running.current) a.cancel();
      running.current = [];
      setRolling(false);
      setLeaving(false);
      if (!open) {
        setRedraws(n => n + 1);
        onClosedRef.current?.();
      }
    };
    if (!box || !content || !canAnimate(instant)) {
      snap();
      return;
    }
    setRolling(true);
    setLeaving(!open);

    rollAfterCommit(() => {
      if (toggles.current !== toggle) return () => {};
      const interrupted = running.current.length > 0;
      if (!interrupted && offScreen(box)) return snap;
      const height = content.getBoundingClientRect().height;
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
          if (!open) onClosedRef.current?.();
        }, () => {  });
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
