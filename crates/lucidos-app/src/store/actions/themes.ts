import { signal } from '@preact/signals';
import { sanitizeResolvedTheme, type ThemeModeMaps } from '@lucidos/appearance';
import { getThemeTokenCatalog, listThemes, type Theme, type ThemeTokenCatalog } from '../../api/client';
import { showConfirm } from '../store';
import { failedIfFresh, setLoadingIfFresh, type Loadable } from '../types';
import { currentThemeMode, paintedThemeMode, setTheme, setThemeMode } from './preferences';

export interface ThemeGallery {
  themes: Theme[];
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
    const safe = themes.map(theme => ({ ...theme, resolved: sanitizeResolvedTheme(theme.resolved) }));
    themeGallery.value = { status: 'loaded', data: { themes: safe, defaults: catalogDefaults(catalog) } };
  } catch (e) {
    if (seq !== loadSeq) return;
    themeGallery.value = failedIfFresh(themeGallery.value, e);
  }
}

export async function pickTheme(theme: Theme): Promise<void> {
  const only = theme.modes.length === 1 ? theme.modes[0] : null;
  const painted = paintedThemeMode.value;
  if (!only || only === painted) return setTheme(theme.id, theme.resolved);

  const followsSystem = currentThemeMode() === 'system'
    ? '\n\nThis device then stops following the system setting.'
    : '';
  const ok = await showConfirm(
    `${theme.name} has no ${painted} mode, so it shows only in ${only} mode.${followsSystem}`,
    `Switch to ${only} mode`,
    { variant: 'default', title: `${theme.name} is ${only} only` },
  );
  if (!ok) return;
  await Promise.all([setThemeMode(only), setTheme(theme.id, theme.resolved)]);
}

export function refreshThemesIfLoaded(): void {
  if (themeGallery.value.status !== 'not-loaded') void loadThemeGallery();
}
