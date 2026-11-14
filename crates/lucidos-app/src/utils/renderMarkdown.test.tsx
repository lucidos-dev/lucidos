// @vitest-environment jsdom
/**
 * The markdown facade before and after the markdown engine lands. The test
 * setup provides the engine to every file, so each test here takes a fresh
 * facade that has none.
 */
import { describe, it, expect, beforeEach } from 'vitest';
import { vi } from 'vitest';

type Facade = typeof import('./renderMarkdown');

let facade: Facade;

beforeEach(async () => {
  vi.resetModules();
  facade = await import('./renderMarkdown');
});

const HOSTILE = [
  '<img src=x onerror=alert(1)>',
  '[click](javascript:alert(1))',
  '<script>alert(1)</script>',
  '"><svg onload=alert(1)>',
].join('\n');

describe('the markdown facade before the engine lands', () => {
  it('has no engine', () => {
    expect(facade.markdownEngine.value).toBeNull();
  });

  for (const name of ['renderMarkdown', 'renderMarkdownInline'] as const) {
    it(`${name} escapes every tag in its input`, () => {
      const html = facade[name](HOSTILE);
      const body = document.createElement('div');
      body.innerHTML = html;
      expect(body.querySelectorAll('img, svg, script, a')).toHaveLength(0);
      expect(body.textContent).toBe(HOSTILE.replace(/\n/g, ''));
    });
  }

  it('keeps line breaks and draws no formatting', () => {
    expect(facade.renderMarkdown('**bold**\nnext')).toBe('<p>**bold**<br>next</p>');
    expect(facade.renderMarkdownInline('*a*\nb')).toBe('*a*<br>b');
  });

  it('formats once the engine is provided, never answering from the plain text', async () => {
    expect(facade.renderMarkdown('**bold**')).not.toContain('<strong>');
    facade.provideMarkdownEngine(await import('./markdownEngine'));
    expect(facade.renderMarkdown('**bold**')).toContain('<strong>bold</strong>');
    expect(facade.renderMarkdownInline('*a*')).toContain('<em>a</em>');
  });
});

describe('a memoised markdown surface', () => {
  it('draws again, formatted, once the engine lands', async () => {
    const { render } = await import('preact');
    const { MarkdownDocument } = await import('../components/files/MarkdownDocument');
    const host = document.createElement('div');
    document.body.append(host);
    render(<MarkdownDocument content={'# Title\n\n**bold**'} location={{ kind: 'workspace', path: 'a.md' }} />, host);
    expect(host.querySelector('strong')).toBeNull();
    expect(host.textContent).toContain('**bold**');

    facade.provideMarkdownEngine(await import('./markdownEngine'));
    await vi.waitFor(() => expect(host.querySelector('strong')?.textContent).toBe('bold'));
    expect(host.querySelector('h1')?.textContent).toBe('Title');
    render(null, host);
    host.remove();
  });
});
