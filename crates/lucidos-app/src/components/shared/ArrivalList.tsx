/**
 * A keyed list whose rows roll in when they arrive and roll out when they
 * leave, with the disclosure roll. A row that arrives where it can be seen
 * wears the *arrival marker*: the navigation focus marker's light, on its own
 * lifecycle (`docs/glossary.md` § Arrival marker).
 *
 * Which rows arrived comes from `useArrivals` (`arrivals.ts`). Only a row that
 * arrives while the list is drawn rolls. A list that mounts with its first
 * arrival marks it without rolling it, because its section rolls in instead.
 *
 * The render callback puts the marker class on the row's own root element.
 * The wash is an inset shadow, which paints above that element's background
 * and so survives a hover tint. On a wrapper it would sit under the row.
 */
import type { ComponentChildren, RefObject } from 'preact';
import { useEffect, useLayoutEffect, useRef, useState } from 'preact/hooks';
import { Disclosure } from './Disclosure';
import { mergeOrder, type Entry } from './arrivals';
import { NAV_FOCUS_FADE_MS, NAV_FOCUS_HOLD_MS, NAV_FOCUS_RAMP_MS } from './focusMarker';
import { DISCLOSURE_MAX_MS } from '../../utils/disclosureMotion';
import { isReducedMotion, scaledDurationMs } from '../../utils/motion';
import { watchUserAction } from '../../utils/userAction';

/** Margin past the longest roll before the marker asks whether its row is on
 *  screen. Mid-roll the clipping box can report a visible row as hidden. */
export const ARRIVAL_CHECK_SLACK_MS = 50;

export function ArrivalList<T>({ items, keyOf, arrived, children }: {
  items: readonly T[];
  keyOf: (item: T) => string;
  arrived: ReadonlySet<string>;
  children: (item: T, markerClass: string | undefined) => ComponentChildren;
}) {
  const drawn = useRef<Entry<T>[]>([]);
  const liveKeys = useRef<ReadonlySet<string>>(new Set());
  const mounted = useRef(false);
  const [, redraw] = useState(0);

  const live = items.map(item => ({ key: keyOf(item), item }));
  liveKeys.current = new Set(live.map(e => e.key));
  const order = mergeOrder(drawn.current, live);
  drawn.current = order;

  useLayoutEffect(() => { mounted.current = true; }, []);

  const drop = (key: string) => {
    if (liveKeys.current.has(key)) return;
    drawn.current = drawn.current.filter(e => e.key !== key);
    redraw(n => n + 1);
  };

  return (
    <>
      {order.map(({ key, item }) => (
        <ArrivalRow
          key={key}
          open={liveKeys.current.has(key)}
          rollIn={mounted.current}
          arrived={arrived.has(key)}
          onGone={() => drop(key)}
          render={(markerClass) => children(item, markerClass)}
        />
      ))}
    </>
  );
}

function ArrivalRow({ open, rollIn, arrived, onGone, render }: {
  open: boolean;
  rollIn: boolean;
  /** True for the render in which the row's key arrives. A row that was
   *  already here never lights. */
  arrived: boolean;
  onGone: () => void;
  render: (markerClass: string | undefined) => ComponentChildren;
}) {
  const box = useRef<HTMLDivElement>(null);
  const [marked, setMarked] = useState(arrived);
  // A row that comes back before it has rolled out is the same instance.
  useEffect(() => { if (arrived) setMarked(true); }, [arrived]);
  const fading = useArrivalMarker(box, marked && open, () => setMarked(false));
  const markerClass = marked ? `arrival-marker${fading ? ' arrival-marker-fading' : ''}` : undefined;
  return (
    <Disclosure open={open} appear={rollIn} onClosed={onGone}>
      <div ref={box}>{render(markerClass)}</div>
    </Disclosure>
  );
}

/** Run the marker on `box` while `active`, and report whether it is fading.
 *
 *  Once the row has landed, it must be on screen in a visible tab, or the
 *  light goes out unseen. Otherwise this is the nav marker's lifecycle: a
 *  ramp, a hold, then a dissolve on the next user action. `retire` runs once
 *  the light is out. */
function useArrivalMarker(box: RefObject<HTMLElement>, active: boolean, retire: () => void): boolean {
  const [fading, setFading] = useState(false);
  const retireRef = useRef(retire);
  retireRef.current = retire;

  useEffect(() => {
    setFading(false);
    const el = box.current;
    if (!active || !el) return;
    let observer: IntersectionObserver | undefined;
    let fadeTimer: ReturnType<typeof setTimeout> | undefined;
    let holdOver = false;
    let dismissQueued = false;

    const dismiss = () => {
      stopWatching();
      if (isReducedMotion()) {
        retireRef.current();
        return;
      }
      setFading(true);
      fadeTimer = setTimeout(() => retireRef.current(), scaledDurationMs(NAV_FOCUS_FADE_MS));
    };
    const stopWatching = watchUserAction(() => {
      if (document.visibilityState !== 'visible') return;
      if (holdOver) dismiss();
      else dismissQueued = true;
    });
    const holdTimer = setTimeout(() => {
      holdOver = true;
      if (dismissQueued) dismiss();
    }, scaledDurationMs(NAV_FOCUS_RAMP_MS) + NAV_FOCUS_HOLD_MS);
    const checkTimer = setTimeout(() => {
      // Read here, not in the observer: a hidden tab runs no observer callback.
      if (document.visibilityState !== 'visible') {
        retireRef.current();
        return;
      }
      if (typeof IntersectionObserver !== 'function') return;
      observer = new IntersectionObserver(([entry]) => {
        observer?.disconnect();
        if (!entry.isIntersecting) retireRef.current();
      });
      observer.observe(el);
    }, scaledDurationMs(DISCLOSURE_MAX_MS) + ARRIVAL_CHECK_SLACK_MS);

    return () => {
      stopWatching();
      observer?.disconnect();
      clearTimeout(holdTimer);
      clearTimeout(checkTimer);
      clearTimeout(fadeTimer);
    };
  }, [active]);

  return fading;
}
