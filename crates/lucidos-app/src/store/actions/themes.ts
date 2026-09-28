/**
 * What the Appearance picker draws: every theme, and the catalog defaults a
 * preview card needs to render a theme on its own. The active theme itself (its
 * maps, its inline layer, its boot cache) lives with the other appearance state
 * in `preferences.ts`.
 */
import { signal } from '@preact/signals';
import { sanitizeResolvedTheme, type ThemeModeMaps } from '@lucidos/appearance';
import { getThemeTokenCatalog, listThemes, type Theme, type ThemeTokenCatalog } from '../../api/client';
import { showConfirm } from '../store';
import { failedIfFresh, setLoadingIfFresh, type Loadable } from '../types';
import { currentThemeMode, paintedThemeMode, setTheme, setThemeMode } from './preferences';

export interface ThemeGallery {
  themes: Theme[];
  /** Every catalog token at its stylesheet default, per mode. A card sets these
   *  before the theme's own map, so it never inherits the page's active theme. */
  defaults: ThemeModeMaps;
}

export const themeGallery = signal<Loadable<ThemeGallery>>({ status: 'not-loaded' });

let loadSeq = 0;

export function catalogDefaults(catalog: ThemeTokenCatalog): ThemeModeMaps {
  const defaults: ThemeModeMaps = { dark: {}, light: {} };
  for (const token of catalog.tokens) {
    defaults.dark[token.name] = token.default.dark;
    defaults.light[token.name] = token.default.light;
  }
  return defaults;
}

export async function loadThemeGallery(): Promise<void> {
  const seq = ++loadSeq;
  setLoadingIfFresh(themeGallery);
  try {
    const [themes, catalog] = await Promise.all([listThemes(), getThemeTokenCatalog()]);
    if (seq !== loadSeq) return;
    // Sanitised once here, since a card lays these maps into an inline style
    // and a workspace theme is a file any app can write.
    const safe = themes.map(theme => ({ ...theme, resolved: sanitizeResolvedTheme(theme.resolved) }));
    themeGallery.value = { status: 'loaded', data: { themes: safe, defaults: catalogDefaults(catalog) } };
  } catch (e) {
    if (seq !== loadSeq) return;
    themeGallery.value = failedIfFresh(themeGallery.value, e);
  }
}

/**
 * Pick a theme from the picker. A single-mode theme paints the default in its
 * other mode. So picking it from that other mode asks first, then switches the
 * device's theme to the theme's mode. Cancel changes nothing.
 */
export async function pickTheme(theme: Theme): Promise<void> {
  const only = theme.modes.length === 1 ? theme.modes[0] : null;
  const painted = paintedThemeMode.value;
  if (!only || only === painted) return setTheme(theme.id);

  const followsSystem = currentThemeMode() === 'system'
    ? '\n\nThis device then stops following the system setting.'
    : '';
  const ok = await showConfirm(
    `${theme.name} has no ${painted} mode, so it shows only in ${only} mode.${followsSystem}`,
    `Switch to ${only} mode`,
    { variant: 'default', title: `${theme.name} is ${only} only` },
  );
  if (!ok) return;
  await Promise.all([setThemeMode(only), setTheme(theme.id)]);
}

/** Re-read after a theme file changed, but only once something asked: nothing
 *  needs the gallery until the picker opens. */
export function refreshThemesIfLoaded(): void {
  if (themeGallery.value.status !== 'not-loaded') void loadThemeGallery();
}
