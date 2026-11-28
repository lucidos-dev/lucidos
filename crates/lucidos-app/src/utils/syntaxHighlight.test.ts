// @vitest-environment jsdom
import './renderMarkdown';
import { expect, it, vi } from 'vitest';

it('keeps the language bundle deferred and replaces escaped fallback after loading', async () => {
  vi.resetModules();
  const facade = await import('./syntaxHighlight');
  expect(facade.syntaxHighlighter.value).toBeNull();
  expect(facade.highlightFileLines('<script>const x = 1</script>', 'js').join('')).toContain('&lt;script&gt;');
  await facade.loadSyntaxHighlighter();
  expect(facade.syntaxHighlighter.value).not.toBeNull();
  expect(facade.highlightFileLines('const x = 1', 'js').join('')).toContain('hljs-keyword');
});

it('does not retry failed highlighting imports from repeated renders', async () => {
  vi.resetModules();
  const preload = vi.fn(async () => false);
  vi.doMock('./lazyComponent', () => ({ lazyModule: () => ({ preload }) }));
  try {
    const facade = await import('./syntaxHighlight');
    facade.highlightFileLines('const x = 1', 'js');
    await Promise.resolve();
    facade.highlightFileLines('const x = 2', 'js');
    expect(preload).toHaveBeenCalledOnce();
    await facade.loadSyntaxHighlighter();
    expect(preload).toHaveBeenCalledTimes(2);
  } finally {
    vi.doUnmock('./lazyComponent');
  }
});
