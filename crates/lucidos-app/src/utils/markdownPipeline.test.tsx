// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { lucidos } from '@lucidos/sdk';
import { renderMarkdown, renderMarkdownInline, sanitizeHtmlFragments } from './markdownEngine';
import { RenderedDiff, renderDiffMarked } from '../components/files/RenderedDiff';
import { SlidesPreview } from '../components/files/SlidesPreview';
import { getChangeFileContent, getRepoFileContent } from '../api/client';

vi.mock('../api/client', async (importOriginal) => ({
  ...await importOriginal<typeof import('../api/client')>(),
  getChangeFileContent: vi.fn(async () => '![diagram](./diagram.png#320x200)'),
  getRepoFileContent: vi.fn(async () => '![diagram](./diagram.png#320x200)'),
}));

const context = vi.hoisted(() => ({ base: '', remote: null as string | null }));
vi.mock('./basePath', () => ({
  get BASE_PATH() { return context.base; },
  get API() { return `${context.base}/api/v1`; },
  get REMOTE_GATEWAY() { return context.remote; },
  get WORKSPACE_ID() { return context.base.slice(1) || null; },
}));

afterEach(() => {
  lucidos.configure({ baseUrl: '' });
  vi.unstubAllGlobals();
  vi.clearAllMocks();
});

const entries = {
  block: (src: string) => renderMarkdown(`![diagram](${src})`, { cache: false }),
  inline: (src: string) => renderMarkdownInline(`![diagram](${src})`),
  fragment: (src: string) => sanitizeHtmlFragments(`<img src="${src}">`),
  diff: (src: string) => renderDiffMarked(`![diagram](${src})`, []),
  slide: (src: string) => {
    const host = document.createElement('div');
    render(<SlidesPreview content={JSON.stringify({ title: 'Deck', slides: [
      { title: 'Slide', content: [{ type: 'html', content: `<img src="${src}">` }] },
    ] })} />, host);
    const html = host.innerHTML;
    render(null, host);
    return html;
  },
};

describe.each(['', '/demo', '/demo@remote'])('HTML pipeline at %s', (base) => {
  it('resolves workspace document images against their folder', () => {
    context.base = base;
    context.remote = base.includes('@') ? 'remote' : null;
    lucidos.configure({ baseUrl: base });
    vi.stubGlobal('location', { origin: 'https://example.test', pathname: `${base}/` });
    const host = document.createElement('div');
    host.innerHTML = renderMarkdown('![diagram](./diagram.png#320x200)', {
      cache: false, document: { kind: 'workspace', path: 'artifacts/reports/readme.md' },
    });
    expect(host.querySelector('img')?.getAttribute('src')).toBe(`${base}/data/artifacts/reports/diagram.png`);
  });

  it.each([null, 'example-change'])('loads and renders diff images at revision %s', async (changeId) => {
    context.base = base;
    context.remote = base.includes('@') ? 'remote' : null;
    vi.stubGlobal('location', { origin: 'https://example.test', pathname: `${base}/` });
    vi.stubGlobal('ResizeObserver', class { observe() {} disconnect() {} });
    const host = document.createElement('div');
    const content = Promise.resolve('![diagram](./diagram.png#320x200)');
    vi.mocked(changeId ? getChangeFileContent : getRepoFileContent).mockReturnValueOnce(content);
    try {
      act(() => {
        render(<RenderedDiff repoId="example" changeId={changeId} gitRef="release" file={{
          path: 'docs/readme.md', status: 'modified', hunks: [{
            old_start: 1, old_count: 1, new_start: 1, new_count: 1,
            lines: [{ type: 'addition', content: '![diagram](./diagram.png#320x200)' }],
          }],
        }} />, host);
      });
      await act(async () => { await content; });
      const expected = changeId
        ? `${base}/api/v1/changes/example-change/file?path=docs%2Fdiagram.png`
        : `${base}/api/v1/repositories/example/file?path=docs%2Fdiagram.png&ref=release`;
      expect(host.querySelector('img')?.getAttribute('src')).toBe(expected);
      expect(host.querySelectorAll('.image-scroll-wrapper')).toHaveLength(1);
      if (changeId) expect(getChangeFileContent).toHaveBeenCalledWith(changeId, 'docs/readme.md');
      else expect(getRepoFileContent).toHaveBeenCalledWith('example', 'docs/readme.md', 'release');
    } finally {
      await act(async () => render(null, host));
    }
  });

  it.each(Object.entries(entries))('%s resolves images exactly once', (_, entry) => {
    context.base = base;
    context.remote = base.includes('@') ? 'remote' : null;
    lucidos.configure({ baseUrl: base });
    vi.stubGlobal('location', { origin: 'https://example.test', pathname: `${base}/` });
    const host = document.createElement('div');
    host.innerHTML = entry('artifacts/diagram%20one.png#320x200');
    expect(host.querySelector('img')?.getAttribute('src')).toBe(`${base}/data/artifacts/diagram%20one.png`);
    expect(host.querySelectorAll('.image-scroll-wrapper')).toHaveLength(1);
    expect(host.querySelector('img')?.style.getPropertyValue('--hint-w')).toBe('320');
    if (context.remote) {
      for (const src of ['/sibling/data/private.png', '/api/v1/control', 'artifacts/%2e%2e/%2e%2e/private.png', 'javascript:alert(1)']) {
        host.innerHTML = entry(src);
        expect(host.querySelector('img')?.hasAttribute('src')).toBe(false);
      }
    }
  });

  it('resolves diff images against the reviewed revision and document', () => {
    context.base = base;
    context.remote = base.includes('@') ? 'remote' : null;
    const html = renderDiffMarked('![diagram](./diagram.png#320x200)', [], [], null,
      { kind: 'repo', repoId: 'example', path: 'docs/readme.md', ref: 'release' });
    const host = document.createElement('div');
    host.innerHTML = html;
    const src = host.querySelector('img')?.getAttribute('src');
    expect(src).toContain(`${base}/api/v1/repositories/example/file?`);
    expect(src).toContain('path=docs%2Fdiagram.png');
    expect(src).toContain('ref=release');
    expect(host.querySelector('img')?.style.getPropertyValue('--hint-w')).toBe('320');
  });
});
