/**
 * Whether theme effects are reduced on this device (ADR 0307).
 *
 * Two inputs, the same shape as `motion.ts`. The device's `theme-effects`
 * preference (`system`, `reduce`, `full`), and two OS signals that count only
 * under `system`: reduced transparency and more contrast. The resolver lives
 * in the appearance contract, so the boot script and app frames agree.
 *
 * CSS reads the `data-theme-effects` attribute `installThemeEffectsAttribute`
 * publishes, and drops every part shadow and filter under `reduce`. The token
 * map stays the same, so a toggle repaints without re-applying the theme.
 */
import { computed, effect, signal } from '@preact/signals';
import {
  THEME_EFFECTS_STORAGE_KEY, MORE_CONTRAST_QUERY, REDUCED_TRANSPARENCY_QUERY,
  themeEffectsAttribute, parseThemeEffects, resolveReducedThemeEffects, type ThemeEffectsPref,
} from '@lucidos/appearance';

/** Held for the page's lifetime: WebKit can collect an unreferenced
 *  `MediaQueryList`, and its `change` listener goes with it. */
function query(media: string): MediaQueryList | null {
  return typeof window !== 'undefined' && typeof window.matchMedia === 'function'
    ? window.matchMedia(media)
    : null;
}
const transparencyQuery = query(REDUCED_TRANSPARENCY_QUERY);
const contrastQuery = query(MORE_CONTRAST_QUERY);

/** The device's preference, seeded from the mirror the boot script reads. */
export const themeEffectsPreference = signal<ThemeEffectsPref>(
  parseThemeEffects(localStorage.getItem(THEME_EFFECTS_STORAGE_KEY)),
);

export const osReducesTransparency = signal(transparencyQuery?.matches === true);
export const osPrefersMoreContrast = signal(contrastQuery?.matches === true);

transparencyQuery?.addEventListener?.('change', (e) => {
  osReducesTransparency.value = e.matches;
});
contrastQuery?.addEventListener?.('change', (e) => {
  osPrefersMoreContrast.value = e.matches;
});

export const reducedThemeEffects = computed(() => resolveReducedThemeEffects(
  themeEffectsPreference.value, osReducesTransparency.value, osPrefersMoreContrast.value,
));

/** Keep `data-theme-effects` on `<html>` in step with the resolved value.
 *  Returns the disposer. Called once from `store/effects.ts`. */
export function installThemeEffectsAttribute(): () => void {
  return effect(() => {
    document.documentElement.setAttribute('data-theme-effects', themeEffectsAttribute(reducedThemeEffects.value));
  });
}
