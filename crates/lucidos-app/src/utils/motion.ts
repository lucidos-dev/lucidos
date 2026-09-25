/**
 * Whether motion is reduced on this device: the client's single answer.
 *
 * Two inputs. The device's `motion` preference (`system`, `reduce`, `full`),
 * and the OS switch, which only counts under `system`. The resolver lives in the
 * appearance contract, so the boot script and app frames reach the same verdict.
 *
 * Script reads `isReducedMotion()`, or `reducedMotion` to subscribe. CSS reads
 * the `data-motion` attribute that `installMotionAttribute` publishes from it.
 * Nothing else may read the media query: a second reader would ignore the
 * in-app choice.
 *
 * The Animation speed scale lives here too, because reduced motion collapses
 * it. Keeping both out of the store lets a lean module scale a timer without
 * loading the store.
 */
import { computed, effect, signal } from '@preact/signals';
import {
  ANIMATION_SPEED_STORAGE_KEY, MOTION_STORAGE_KEY, REDUCED_MOTION_QUERY, durationScaleFor,
  motionAttribute, parseAnimationSpeed, parseMotion, resolveReducedMotion, speedMultiplierFor,
  type MotionPref,
} from '@lucidos/appearance';

/** Held for the page's lifetime. WebKit can collect an unreferenced
 *  `MediaQueryList`, and its `change` listener goes with it. */
const osQuery: MediaQueryList | null =
  typeof window !== 'undefined' && typeof window.matchMedia === 'function'
    ? window.matchMedia(REDUCED_MOTION_QUERY)
    : null;

/** The device's preference. Seeded from the mirror the boot script also reads,
 *  so the first render agrees with the first paint. */
export const motionPreference = signal<MotionPref>(
  parseMotion(localStorage.getItem(MOTION_STORAGE_KEY)),
);

/** The OS reduce-motion switch, kept live by its `change` event. */
export const osReducesMotion = signal(osQuery?.matches === true);

osQuery?.addEventListener?.('change', (e) => {
  osReducesMotion.value = e.matches;
});

export const reducedMotion = computed(
  () => resolveReducedMotion(motionPreference.value, osReducesMotion.value),
);

/** For imperative callers: an animation about to start, a scroll behaviour. */
export function isReducedMotion(): boolean {
  return reducedMotion.value;
}

/** `smooth` unless motion is reduced. For `scrollTo` / `scrollIntoView`. */
export function scrollBehavior(): ScrollBehavior {
  return reducedMotion.value ? 'auto' : 'smooth';
}

/** Keep `data-motion` on `<html>` in step with the resolved value. Returns the
 *  disposer. Called once from `store/effects.ts`. */
export function installMotionAttribute(): () => void {
  return effect(() => {
    document.documentElement.setAttribute('data-motion', motionAttribute(reducedMotion.value));
  });
}

// --- Animation speed ---

/** The diagnostic Animation speed slider's position, -10..10, 0 = normal. */
export const animationSpeed = signal(
  parseAnimationSpeed(localStorage.getItem(ANIMATION_SPEED_STORAGE_KEY)),
);

/** Slider position (-10..10) → speed multiplier (0.1x..10x) via 10^(v/10). */
export const speedMultiplier = computed(() => speedMultiplierFor(animationSpeed.value));

/** What every animated duration is MULTIPLIED by: the reciprocal of the speed,
 *  so 10x speed is a 0.1 scale. Reduced motion collapses it to a near-zero
 *  constant whatever the slider says. Every scaled transition then finishes at
 *  once, and so does every timer mirroring one. It exists beside the
 *  multiplier so one name root crosses both layers:
 *
 *    - CSS reads it as `var(--duration-scale)`, published onto :root by
 *      store/effects.ts and folded into every `--duration-*` token in
 *      styles/global/base.css. That is what lets the slider reach a plain CSS
 *      transition at all.
 *    - TS reads it through `scaledDurationMs` for a timer that must outlive
 *      one of those transitions, and directly for a Web Animations duration
 *      (useFlipAnimation).
 *
 *  1 at the slider's centre, so a user who never touches it sees today's
 *  timings exactly. */
export const durationScale = computed(() => durationScaleFor(animationSpeed.value, reducedMotion.value));

/** A base duration in ms, scaled to the current animation speed.
 *
 *  For a TS timer that MIRRORS a CSS duration, such as keeping an element
 *  mounted through its own fade. Pass the 1x duration of the CSS it mirrors.
 *  Add any safety slack OUTSIDE the call, since slack is a fixed margin rather
 *  than animation. So `scaledDurationMs(PANE_TRANSITION_MS) + 100` is the
 *  shape, never `scaledDurationMs(PANE_TRANSITION_MS + 100)`.
 *
 *  Scaling the CSS without scaling these desyncs the pair. At 0.1x the
 *  drawer's width transition runs 3s while an unscaled 350ms timer unmounts
 *  its list, so the drawer blanks and then slides shut empty. */
export function scaledDurationMs(baseMs: number): number {
  return baseMs * durationScale.value;
}
