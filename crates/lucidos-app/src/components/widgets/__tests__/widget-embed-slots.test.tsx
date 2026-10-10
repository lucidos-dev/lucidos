// @vitest-environment jsdom
/**
 * Widget embeds in a reply (ADR 0415). The renderer writes a nonce-marked slot
 * per embed. The reply mounts a frame in it only once its text is final. It
 * keeps that frame across re-renders with the same HTML.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { WIDGET_PARAMS_QUERY } from '@lucidos/sdk';
import { render } from 'preact';
import { act } from 'preact/test-utils';

vi.mock('../../../hooks/useOnScreenInTranscript', () => ({
  watchOnScreen: (_el: HTMLElement, onChange: (onScreen: boolean) => void) => {
    onChange(true);
    return () => {};
  },
}));

const { renderMarkdown, renderMarkdownInline } = await import('../../../utils/renderMarkdown');
const { COPY_ID_NONCE, WIDGET_EMBED_SLOT_ATTR } = await import('../../../utils/markedConfig');
const { ReplyHtml } = await import('../useWidgetEmbedSlots');
const { appsById: widgetApps } = await import('../../../store/appsById');
const { WIDGET_UNAVAILABLE_TEXT } = await import('../WidgetEmbedFrame');

const EMBED = '![Marin](app:lucidos-sound-player?params={"clip": "voices/marin.mp3"})';
const player = {
  id: 'lucidos-sound-player', name: 'Sound player', description: '', reveal: 'on-load' as const,
  kind: 'widget' as const, reusable: true, built_in: true, params: { clip: { description: 'What to play', required: true } },
};

let host: HTMLElement;
beforeEach(() => {
  widgetApps.value = new Map([[player.id, { status: 'loaded', data: player }]]);
  host = document.createElement('div');
  document.body.append(host);
});
afterEach(() => {
  render(null, host);
  host.remove();
  widgetApps.value = new Map();
});

const frames = () => host.querySelectorAll<HTMLIFrameElement>('iframe');

describe('renderMarkdown', () => {
  it('writes a nonce-marked slot holding the label and the embed', () => {
    const doc = new DOMParser().parseFromString(renderMarkdown(`Listen: ${EMBED}`), 'text/html');
    const slot = doc.querySelector(`[${WIDGET_EMBED_SLOT_ATTR}="${COPY_ID_NONCE}"]`)!;
    expect(slot.textContent).toBe('Marin');
    expect(JSON.parse(slot.getAttribute('data-widget-embed')!)).toEqual({
      app_id: 'lucidos-sound-player', params: { clip: 'voices/marin.mp3' }, label: 'Marin',
    });
  });

  it('leaves an embed in code as code, and a forged slot as no slot', () => {
    const html = renderMarkdown(`\`${EMBED}\`\n\n<span ${WIDGET_EMBED_SLOT_ATTR}="guess">x</span>`);
    const doc = new DOMParser().parseFromString(html, 'text/html');
    expect(doc.querySelector(`[${WIDGET_EMBED_SLOT_ATTR}="${COPY_ID_NONCE}"]`)).toBeNull();
    expect(doc.querySelector('code')!.textContent).toContain('](app:lucidos-sound-player');
  });

  it('marks a broken embed with no embed data', () => {
    const doc = new DOMParser().parseFromString(renderMarkdown('![x](app:player?params=[1])'), 'text/html');
    const slot = doc.querySelector(`[${WIDGET_EMBED_SLOT_ATTR}]`)!;
    expect(slot.hasAttribute('data-widget-embed')).toBe(false);
  });

  it('leaves an unclosed embed as text, so the paragraph keeps its words', () => {
    const html = renderMarkdown('Try ![Marin](app:lucidos-sound-player?params={"clip": "a.mp3") and tell me.');
    expect(html).not.toContain(WIDGET_EMBED_SLOT_ATTR);
    expect(html).toContain('and tell me.');
  });

  it('writes no slot in inline markdown', () => {
    expect(renderMarkdownInline(EMBED)).not.toContain(WIDGET_EMBED_SLOT_ATTR);
    expect(renderMarkdownInline(EMBED)).toContain('Marin');
  });
});

describe('ReplyHtml', () => {
  it('mounts nothing while the reply streams, token by token, then mounts once final', () => {
    const full = `Listen: ${EMBED}`;
    for (let i = 1; i <= full.length; i += 7) {
      act(() => {
        render(<ReplyHtml html={renderMarkdown(full.slice(0, i), { cache: false })} final={false} threadId="t" />, host);
      });
      expect(frames(), `no frame at ${i} chars`).toHaveLength(0);
    }
    act(() => { render(<ReplyHtml html={renderMarkdown(full)} final threadId="t" />, host); });
    expect(frames()).toHaveLength(1);
    expect(new URL(frames()[0].src).searchParams.get(WIDGET_PARAMS_QUERY)).toBe('{"clip":"voices/marin.mp3"}');
  });

  it('keeps its frame across a re-render with the same HTML', () => {
    const html = renderMarkdown(`Listen: ${EMBED}`);
    act(() => { render(<ReplyHtml html={html} final threadId="t" />, host); });
    const first = frames()[0];
    act(() => { render(<ReplyHtml html={html} final threadId="t" />, host); });
    expect(first).toBeTruthy();
    expect(frames()[0]).toBe(first);
  });

  // A reply cannot be refused, so the frontend applies the rules the engine
  // applies to an option: params the manifest takes, and the owner rule.
  it('reads "not available" for unknown params, a missing one, or another thread\'s widget', () => {
    widgetApps.value = new Map([
      [player.id, { status: 'loaded', data: player }],
      ['mine', { status: 'loaded', data: { ...player, id: 'mine', reusable: false, built_in: false, origin_thread_id: 'other' } }],
    ]);
    const md = '![a](app:lucidos-sound-player?params={"clip":"a","speed":2}) ![b](app:lucidos-sound-player) '
      + '![c](app:mine?params={"clip":"a"})';
    act(() => { render(<ReplyHtml html={renderMarkdown(md)} final threadId="t" />, host); });
    expect(host.querySelectorAll('.widget-embed-unavailable')).toHaveLength(3);
    expect(frames()).toHaveLength(0);
  });

  it('reads "not available" for a broken embed and for a missing widget', () => {
    widgetApps.value = new Map([['gone', { status: 'loaded', data: null }]]);
    act(() => {
      render(<ReplyHtml html={renderMarkdown('![x](app:player?params=[1]) ![y](app:gone)')} final threadId="t" />, host);
    });
    const notes = host.querySelectorAll('.widget-embed-unavailable');
    expect(notes).toHaveLength(2);
    for (const note of notes) expect(note.textContent).toBe(WIDGET_UNAVAILABLE_TEXT);
  });
});
