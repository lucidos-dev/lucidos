import { API, json } from './_core';
import type { ResolvedTheme } from '@lucidos/appearance';

export const THEME_FAMILIES = ['blue', 'violet', 'warm', 'neutral'] as const;
export type ThemeFamily = typeof THEME_FAMILIES[number];

export interface Theme {
  id: string;
  source: 'built-in' | 'workspace';
  name: string;
  description?: string;
  author?: string;
  credit?: string;
  family?: ThemeFamily;
  modes: Array<'dark' | 'light'>;
  resolved: ResolvedTheme;
}

export async function listThemes(): Promise<Theme[]> {
  const { themes } = await json<{ themes: Theme[] }>(`${API}/themes`);
  return themes;
}

export async function getTheme(id: string): Promise<Theme> {
  return json<Theme>(`${API}/theme?id=${encodeURIComponent(id)}`);
}

export interface ThemeToken {
  name: string;
  group: string;
  kind: string;
  label: string;
  description: string;
  frames: boolean;
  default: { dark: string; light: string };
  derive?: { value: string; seeds: string[] };
}

export interface ThemeTokenCatalog {
  seeds: string[];
  groups: Array<{ id: string; label: string; description: string }>;
  tokens: ThemeToken[];
}

export async function getThemeTokenCatalog(): Promise<ThemeTokenCatalog> {
  return json<ThemeTokenCatalog>(`${API}/themes/tokens`);
}
