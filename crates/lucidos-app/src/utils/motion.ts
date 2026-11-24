import { computed, effect, signal } from '@preact/signals';
import {
  ANIMATION_SPEED_STORAGE_KEY, MOTION_STORAGE_KEY, REDUCED_MOTION_QUERY, durationScaleFor,
  motionAttribute, parseAnimationSpeed, parseMotion, resolveReducedMotion, speedMultiplierFor,
  type MotionPref,
} from '@lucidos/appearance';

const osQuery: MediaQueryList | null =
  typeof window !== 'undefined' && typeof window.matchMedia === 'function'
    ? window.matchMedia(REDUCED_MOTION_QUERY)
    : null;

export const motionPreference = signal<MotionPref>(
  parseMotion(localStorage.getItem(MOTION_STORAGE_KEY)),
);

export const osReducesMotion = signal(osQuery?.matches === true);

osQuery?.addEventListener?.('change', (e) => {
  osReducesMotion.value = e.matches;
});

export const reducedMotion = computed(
  () => resolveReducedMotion(motionPreference.value, osReducesMotion.value),
);

export function isReducedMotion(): boolean {
  return reducedMotion.value;
}

export function scrollBehavior(): ScrollBehavior {
  return reducedMotion.value ? 'auto' : 'smooth';
}

export function installMotionAttribute(): () => void {
  return effect(() => {
    document.documentElement.setAttribute('data-motion', motionAttribute(reducedMotion.value));
  });
}


export const animationSpeed = signal(
  parseAnimationSpeed(localStorage.getItem(ANIMATION_SPEED_STORAGE_KEY)),
);

export const speedMultiplier = computed(() => speedMultiplierFor(animationSpeed.value));

export const durationScale = computed(() => durationScaleFor(animationSpeed.value, reducedMotion.value));

export function scaledDurationMs(baseMs: number): number {
  return baseMs * durationScale.value;
}
