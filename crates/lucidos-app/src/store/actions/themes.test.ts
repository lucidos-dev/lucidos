import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import * as apiClient from '../../api/client';
import type { Theme } from '../../api/client';
import { loadThemeGallery, themeGallery } from './themes';

function workspaceTheme(id: string, dark: Record<string, string>): Theme {
  return { id, source: 'workspace', name: id, modes: ['dark'], resolved: { dark, light: {}, fonts: {}, workspace_fonts: [] } };
}

beforeEach(() => {
  themeGallery.value = { status: 'not-loaded' };
  vi.spyOn(apiClient, 'getThemeTokenCatalog').mockResolvedValue({ seeds: [], groups: [], tokens: [] } as never);
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe('loadThemeGallery', () => {
  it('strips style-injecting values from a workspace theme before the gallery draws it', async () => {
    vi.spyOn(apiClient, 'listThemes').mockResolvedValue([
      workspaceTheme('hostile', {
        '--bg': 'red; background: url(https://example.test/a.png)',
        '--accent': 'red',
      }),
    ]);
    await loadThemeGallery();
    const status = themeGallery.value.status;
    expect(status).toBe('loaded');
    if (themeGallery.value.status !== 'loaded') return;
    const dark = themeGallery.value.data.themes[0].resolved.dark;
    expect(dark['--bg']).toBeUndefined();
    expect(dark['--accent']).toBe('red');
  });
});
