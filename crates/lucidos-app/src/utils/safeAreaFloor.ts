/** Restores the safe-area insets an iOS home-screen app loses.
 *
 *  After a full-screen interruption, such as a phone call, WebKit can resolve
 *  `env(safe-area-inset-*)` to 0, on every side or possibly the top alone. The
 *  loss may outlast a relaunch, so the last real reading is kept in storage. The
 *  header then sits under the clock and the prompt under the home indicator.
 *  CSS cannot override `env()`, so every inset reads `var(--safe-area-*)`
 *  (styles/global/base.css), which takes the larger of `env()` and the floor
 *  this module publishes.
 *
 *  Registered in `docs/temporary-measures.md` § 1, "Safe-area floor". */
import { isIOSPwa } from './platform';

export type SafeAreaSide = 'top' | 'right' | 'bottom' | 'left';
export type SafeAreaInsets = Record<SafeAreaSide, number>;

export const SAFE_AREA_SIDES: readonly SafeAreaSide[] = ['top', 'right', 'bottom', 'left'];

/** The sides a status bar, notch or island owns. Within one viewport shape
 *  their insets never drop to 0 unless WebKit lost them. The bottom is not
 *  one: the keyboard can zero it. */
const HARDWARE_SIDES: readonly SafeAreaSide[] = ['top', 'right', 'left'];

/** Fold one `env()` reading into the memory for its viewport shape.
 *
 *  WebKit loses insets by zeroing them, so a lost reading takes insets away and
 *  adds none: a hardware side reads 0 against a remembered inset, and no side
 *  gains one. It publishes the memory as the floor and leaves it alone. Any
 *  other reading replaces the memory whole, which also clears a memory filed
 *  under the wrong shape mid-rotation. */
export function foldSafeAreaReading(
  remembered: SafeAreaInsets | undefined,
  reading: SafeAreaInsets,
): { remembered: SafeAreaInsets | undefined; floor: SafeAreaInsets | null } {
  if (remembered) {
    const dropped = HARDWARE_SIDES.some((side) => reading[side] === 0 && remembered[side] > 0);
    const gained = SAFE_AREA_SIDES.some((side) => reading[side] > 0 && remembered[side] === 0);
    if (dropped && !gained) return { remembered, floor: remembered };
  }
  const allZero = SAFE_AREA_SIDES.every((side) => reading[side] === 0);
  return { remembered: allZero ? remembered : reading, floor: null };
}

const STORAGE_KEY = 'lucidos-safe-area-insets';
/** Enough for both orientations of every window size a phone or tablet uses. */
const MAX_REMEMBERED_SHAPES = 8;

function isInsets(value: unknown): value is SafeAreaInsets {
  if (typeof value !== 'object' || value === null) return false;
  const record = value as Record<string, unknown>;
  return SAFE_AREA_SIDES.every((side) => {
    const inset = record[side];
    return typeof inset === 'number' && Number.isFinite(inset) && inset >= 0;
  });
}

/** The stored insets per shape, skipping any entry that is not a full set.
 *
 *  Storage failures only warn. The floor runs without user intent, so a toast
 *  would report nothing the user did. The memory still works for this launch,
 *  and the next real reading rewrites the stored copy. */
function loadShapes(storage: Pick<Storage, 'getItem'>): Map<string, SafeAreaInsets> {
  const shapes = new Map<string, SafeAreaInsets>();
  let parsed: unknown;
  try {
    const raw = storage.getItem(STORAGE_KEY);
    parsed = raw === null ? null : JSON.parse(raw);
  } catch (err) {
    console.warn('Safe-area floor: ignoring unreadable stored insets', err);
    return shapes;
  }
  if (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed)) return shapes;
  for (const [shape, insets] of Object.entries(parsed)) {
    if (isInsets(insets)) shapes.set(shape, insets);
  }
  return shapes;
}

/** The remembered insets per viewport shape, kept in storage so a cold launch
 *  that starts in the lost state still has a floor. */
export function createSafeAreaMemory(storage: Pick<Storage, 'getItem' | 'setItem'>): {
  get(shape: string): SafeAreaInsets | undefined;
  set(shape: string, insets: SafeAreaInsets): void;
} {
  // A Map iterates in insertion order, so the first key is the stalest shape.
  const shapes = loadShapes(storage);
  let newest = [...shapes.keys()].pop();
  return {
    get: (shape) => shapes.get(shape),
    set(shape, insets) {
      const known = shapes.get(shape);
      const unchanged = known && SAFE_AREA_SIDES.every((side) => known[side] === insets[side]);
      if (unchanged && shape === newest) return;
      newest = shape;
      shapes.delete(shape);
      shapes.set(shape, insets);
      for (const stalest of shapes.keys()) {
        if (shapes.size <= MAX_REMEMBERED_SHAPES) break;
        shapes.delete(stalest);
      }
      try {
        storage.setItem(STORAGE_KEY, JSON.stringify(Object.fromEntries(shapes)));
      } catch (err) {
        // Best effort, as in loadShapes: the memory still works for this launch.
        console.warn('Safe-area floor: could not store the insets', err);
      }
    },
  };
}

/** The viewport shape a set of insets belongs to. A rotation moves the notch to
 *  another side, and an iPad window resize can change the insets too. */
export function viewportShapeKey(opts: { orientation: string | undefined; width: number; height: number }): string {
  const orientation = opts.orientation ?? (opts.width > opts.height ? 'landscape' : 'portrait');
  return `${orientation}:${opts.width}`;
}

/** Frames to keep re-reading after a return to the foreground. WebKit settles
 *  the insets a few frames after the page shows, and posts no event for it. */
const WAKE_RECHECK_FRAMES = 30;

let installed = false;

export function installSafeAreaFloor(): void {
  if (installed || !isIOSPwa()) return;
  installed = true;

  const probe = document.createElement('div');
  probe.setAttribute('aria-hidden', 'true');
  // Reads raw `env()`, never the floored var, or the floor would feed itself.
  probe.style.cssText = 'position:fixed;top:0;left:0;width:0;height:0;visibility:hidden;pointer-events:none;'
    + 'padding:env(safe-area-inset-top,0px) env(safe-area-inset-right,0px) '
    + 'env(safe-area-inset-bottom,0px) env(safe-area-inset-left,0px)';
  document.body.appendChild(probe);

  const memory = createSafeAreaMemory(localStorage);
  const root = document.documentElement.style;

  function read(): SafeAreaInsets {
    const cs = getComputedStyle(probe);
    return {
      top: parseFloat(cs.paddingTop) || 0,
      right: parseFloat(cs.paddingRight) || 0,
      bottom: parseFloat(cs.paddingBottom) || 0,
      left: parseFloat(cs.paddingLeft) || 0,
    };
  }

  function sync() {
    const key = viewportShapeKey({
      orientation: screen.orientation?.type,
      width: window.innerWidth,
      height: window.innerHeight,
    });
    const { remembered, floor } = foldSafeAreaReading(memory.get(key), read());
    if (remembered) memory.set(key, remembered);
    for (const side of SAFE_AREA_SIDES) {
      if (floor) root.setProperty(`--safe-area-floor-${side}`, `${floor[side]}px`);
      else root.removeProperty(`--safe-area-floor-${side}`);
    }
  }

  let recheckFrames = 0;
  let recheckId: number | null = null;
  function recheck() {
    sync();
    recheckId = --recheckFrames > 0 ? requestAnimationFrame(recheck) : null;
  }
  function onWake() {
    if (document.visibilityState !== 'visible') return;
    recheckFrames = WAKE_RECHECK_FRAMES;
    if (recheckId === null) recheckId = requestAnimationFrame(recheck);
    sync();
  }

  // The probe's box is its insets, so any change to `env()` resizes it.
  new ResizeObserver(sync).observe(probe, { box: 'border-box' });
  window.addEventListener('resize', sync);
  window.addEventListener('orientationchange', sync);
  document.addEventListener('visibilitychange', onWake);
  window.addEventListener('pageshow', onWake);
  sync();
}
