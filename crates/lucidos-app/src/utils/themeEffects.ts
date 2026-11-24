import { computed, effect, signal } from '@preact/signals';
import {
  THEME_EFFECTS_STORAGE_KEY, MORE_CONTRAST_QUERY, REDUCED_TRANSPARENCY_QUERY,
  themeEffectsAttribute, parseThemeEffects, resolveReducedThemeEffects, type ThemeEffectsPref,
} from '@lucidos/appearance';

function query(media: string): MediaQueryList | null {
  return typeof window !== 'undefined' && typeof window.matchMedia === 'function'
    ? window.matchMedia(media)
    : null;
}
const transparencyQuery = query(REDUCED_TRANSPARENCY_QUERY);
const contrastQuery = query(MORE_CONTRAST_QUERY);

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

export function installThemeEffectsAttribute(): () => void {
  return effect(() => {
    document.documentElement.setAttribute('data-theme-effects', themeEffectsAttribute(reducedThemeEffects.value));
  });
}
