// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { useMemo, useState } from 'preact/hooks';
import { useMermaidBlocks } from './useMermaidBlocks';
import { renderMarkdown } from '../../utils/markdownEngine';
import { loadMermaidLibrary } from '../../utils/mermaidLibrary';
vi.mock('../../utils/mermaidLibrary', () => ({ loadMermaidLibrary: vi.fn(async () => '/* mermaid */') }));
vi.mock('../../utils/cssColor', () => ({ resolvedTokenHex: vi.fn(() => '#123456') }));
const host = document.createElement('div');
document.body.append(host);
afterEach(() => { act(() => render(null, host)); vi.clearAllMocks(); });
function Reply({ md, final = true }: { md: string; final?: boolean }) {
  const [el, ref] = useState<HTMLDivElement | null>(null);
  const html = useMemo(() => renderMarkdown(md), [md]);
  useMermaidBlocks(el, html, final);
  return <div ref={ref} dangerouslySetInnerHTML={{ __html: html }} />;
}
const md = '```mermaid\nflowchart LR\n A-->B\n```';
async function mount(text = md, final = true) {
  await import('./mermaidBlocks');
  await act(async () => { render(<Reply md={text} final={final} />, host); });
  if (final && text.includes('```mermaid')) {
    await vi.waitFor(() => expect(host.querySelector('iframe')).not.toBeNull());
  }
}
describe('final reply Mermaid blocks', () => {
  it('keeps streaming source and ordinary code unchanged', async () => {
    await mount(md, false);
    expect(host.querySelector('iframe')).toBeNull();
    expect(host.querySelector('pre')?.textContent).toContain('A-->B');
    await mount('```ts\nconst x = 1\n```');
    expect(host.querySelector('iframe')).toBeNull();
  });
  it('creates isolated frames and switches independent blocks without losing source', async () => {
    await mount(md + '\n\n' + md);
    const frames = host.querySelectorAll('iframe');
    expect(frames).toHaveLength(2);
    expect(frames[0].getAttribute('sandbox')).toBe('allow-scripts');
    await vi.waitFor(() => {
      for (const frame of frames) expect(frame.srcdoc).toContain('const channel');
    });
    const buttons = host.querySelectorAll<HTMLButtonElement>('.segmented-btn');
    act(() => buttons[1].click());
    const blocks = host.querySelectorAll('[data-copy-code]');
    expect(blocks[0].querySelector('pre')?.hidden).toBe(false);
    expect(blocks[0].querySelector('iframe')?.hidden).toBe(true);
    expect(blocks[1].querySelector('pre')?.hidden, blocks[1].outerHTML).toBe(true);
    expect(blocks[0].querySelector('pre code')?.textContent).toBe('flowchart LR\n A-->B');
    expect(blocks[0].querySelector('.copy-btn')).not.toBeNull();
  });
  it('does not enhance authored lookalike wrappers', async () => {
    await mount('<div class="code-block-wrapper" data-copy-code><div class="code-block-header"><span class="code-block-lang">mermaid</span></div><pre><code>bad</code></pre></div>');
    expect(host.querySelector('iframe')).toBeNull();
  });
  it('shows source and an error when loading fails', async () => {
    vi.mocked(loadMermaidLibrary).mockRejectedValueOnce(new Error('offline'));
    await mount();
    await vi.waitFor(() => expect(host.querySelector('pre')?.hidden).toBe(false));
    expect(host.querySelector('[role="status"]')?.textContent).toContain('Could not render');
    expect(host.querySelector<HTMLButtonElement>('.segmented-btn')?.disabled).toBe(true);
  });
  it('discards delayed loads after replacement or unmount', async () => {
    let finish!: (library: string) => void;
    vi.mocked(loadMermaidLibrary).mockReturnValueOnce(new Promise((resolve) => { finish = resolve; }));
    await mount();
    const frame = host.querySelector('iframe')!;
    await mount('ordinary reply');
    await act(async () => finish('late library'));
    expect(frame.hasAttribute('srcdoc')).toBe(false);
    expect(host.querySelector('iframe')).toBeNull();
    expect(host.textContent).toContain('ordinary reply');
  });
  it('accepts render failures only from the matching frame and channel', async () => {
    await mount();
    const frame = host.querySelector('iframe')!;
    await vi.waitFor(() => expect(frame.srcdoc).toContain('const channel'));
    const channel = JSON.parse(frame.srcdoc.match(/const channel = ("[^"]+")/)![1]);
    dispatchEvent(new MessageEvent('message', { data: { channel, status: 'error' }, source: window }));
    expect(host.querySelector('pre')?.hidden).toBe(true);
    dispatchEvent(new MessageEvent('message', { data: { channel: 'wrong', status: 'error' }, source: frame.contentWindow }));
    expect(host.querySelector('pre')?.hidden).toBe(true);
    dispatchEvent(new MessageEvent('message', { data: { channel, status: 'error' }, source: frame.contentWindow }));
    expect(host.querySelector('pre')?.hidden).toBe(false);
    expect(host.querySelector('[role="status"]')?.textContent).toContain('Could not render');
    dispatchEvent(new MessageEvent('message', { data: { channel, status: 'rendered' }, source: frame.contentWindow }));
    expect(host.querySelector<HTMLElement>('[role="status"]')?.hidden).toBe(false);
  });
  it('defers rendering hidden frames and sends resolved theme colors on return', async () => {
    await mount();
    const frame = host.querySelector('iframe')!;
    await vi.waitFor(() => expect(frame.srcdoc).toContain('const channel'));
    const channel = JSON.parse(frame.srcdoc.match(/const channel = ("[^"]+")/)![1]);
    const post = vi.spyOn(frame.contentWindow!, 'postMessage');
    act(() => host.querySelectorAll<HTMLButtonElement>('.segmented-btn')[1].click());
    dispatchEvent(new MessageEvent('message', { data: { channel, status: 'ready' }, source: frame.contentWindow }));
    expect(post).not.toHaveBeenCalled();
    act(() => host.querySelectorAll<HTMLButtonElement>('.segmented-btn')[0].click());
    expect(post).toHaveBeenCalledWith(expect.objectContaining({ theme: expect.objectContaining({ primaryColor: '#123456' }) }), '*');
    post.mockRestore();
  });
});
