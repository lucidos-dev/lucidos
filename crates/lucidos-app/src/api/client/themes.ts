import { API, json } from './_core';
import type { ResolvedTheme } from '@lucidos/appearance';

/** Mirrors `ThemeFamily` in the engine's `core/themes/mod.rs`, in its order. */
export const THEME_FAMILIES = ['blue', 'violet', 'warm', 'neutral'] as const;
export type ThemeFamily = typeof THEME_FAMILIES[number];

/** A *theme*: a named set of design-token values, as the engine serves it.
 *  Mirrors `Theme` in the engine's `core/themes/mod.rs`. */
export interface Theme {
  /** The file stem under `data/themes/`, or a built-in id. */
  id: string;
  source: 'built-in' | 'workspace';
  name: string;
  description?: string;
  author?: string;
  credit?: string;
  family?: ThemeFamily;
  /** The theme modes the theme has its own map for. Empty means it only
   *  retunes shared tokens (radii, say), in both modes. */
  modes: Array<'dark' | 'light'>;
  /** The map each mode paints, derivation already applied by the engine,
   *  and the fonts the theme suggests. */
  resolved: ResolvedTheme;
}

/** See engine `GET /api/v1/themes`. Built-ins first, then the workspace's. */
export async function listThemes(): Promise<Theme[]> {
  const { themes } = await json<{ themes: Theme[] }>(`${API}/themes`);
  return themes;
}

/** See engine `GET /api/v1/theme?id=`. */
export async function getTheme(id: string): Promise<Theme> {
  return json<Theme>(`${API}/theme?id=${encodeURIComponent(id)}`);
}

/** One tunable token. Mirrors an entry of the engine's `theme-tokens.json`. */
export interface ThemeToken {
  name: string;
  group: string;
  kind: string;
  label: string;
  description: string;
  /** Whether app frames define this token too. */
  frames: boolean;
  default: { dark: string; light: string };
  /** Filled in when a theme sets one of `seeds` but not this token. */
  derive?: { value: string; seeds: string[] };
}

/** Everything a theme can tune. See engine `GET /api/v1/themes/tokens`. */
export interface ThemeTokenCatalog {
  seeds: string[];
  groups: Array<{ id: string; label: string; description: string }>;
  tokens: ThemeToken[];
}

export async function getThemeTokenCatalog(): Promise<ThemeTokenCatalog> {
  return json<ThemeTokenCatalog>(`${API}/themes/tokens`);
}
