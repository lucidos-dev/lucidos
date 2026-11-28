// @vitest-environment jsdom
import './renderMarkdown';
import { expect, it, vi } from 'vitest';

it('imports the markdown facade without loading parser or sanitizer', async () => {
  vi.resetModules();
  const engine = await import('./markdownEngine');
  expect(engine.markdownLibrariesReady.value).toBe(false);
  expect(engine.sanitizeHtmlFragments('<img src=x onerror=alert(1)>')).not.toContain('<img');
  await engine.loadMarkdownLibraries();
  expect(engine.markdownLibrariesReady.value).toBe(true);
  expect(engine.renderMarkdown('**ready**')).toContain('<strong>ready</strong>');
  expect(engine.sanitizeHtmlFragments('<img src=x onerror=alert(1)>')).not.toContain('onerror=');
});

it('reports a handled library failure instead of exposing an unready parser', async () => {
  vi.resetModules();
  const preload = vi.fn(async () => false);
  vi.doMock('./lazyComponent', () => ({
    lazyModule: () => ({ preload }),
  }));
  try {
    const engine = await import('./markdownEngine');
    expect(await engine.loadMarkdownLibraries()).toBe(false);
    expect(engine.markdownLibrariesReady.value).toBe(false);
    engine.sanitizeHtmlFragments('first render');
    engine.sanitizeHtmlFragments('next render');
    expect(preload).toHaveBeenCalledOnce();
    await engine.loadMarkdownLibraries();
    expect(preload).toHaveBeenCalledTimes(2);
  } finally {
    vi.doUnmock('./lazyComponent');
  }
});
