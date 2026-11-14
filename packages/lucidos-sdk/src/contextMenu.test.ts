// @vitest-environment jsdom
import { describe, it, expect, vi } from 'vitest';
import { shouldSuppressContextMenu as decide, selectionCovers, opensContextMenu, textAt } from './contextMenu';

function el(html: string, pick = '[data-pick]'): Element {
  const host = document.createElement('div');
  host.innerHTML = html;
  document.body.appendChild(host);
  return host.querySelector(pick) ?? host.firstElementChild!;
}

const facts = (target: EventTarget | null, over: Partial<{ altKey: boolean; defaultPrevented: boolean }> = {}) =>
  ({ target, altKey: false, defaultPrevented: false, ...over });

/** The decision with text drawn under the pointer, unless told otherwise. */
const shouldSuppressContextMenu = (f: ReturnType<typeof facts>, selectionAtPress: boolean, text = true) =>
  decide(f, selectionAtPress, () => text);

describe('shouldSuppressContextMenu', () => {
  it('suppresses the menu over empty chrome', () => {
    expect(shouldSuppressContextMenu(facts(el('<header><span data-pick>Lucidos</span></header>')), false)).toBe(true);
  });

  it('suppresses over a plain button', () => {
    expect(shouldSuppressContextMenu(facts(el('<button data-pick>Apply</button>')), false)).toBe(true);
  });

  it.each([
    ['a text field', '<input data-pick>'],
    ['a textarea', '<textarea data-pick></textarea>'],
    ['an editable region', '<div contenteditable="true"><p data-pick>x</p></div>'],
    ['a link', '<a href="https://example.com"><span data-pick>docs</span></a>'],
    ['an image', '<img data-pick>'],
  ])('keeps the native menu anywhere on %s', (_label, html) => {
    expect(shouldSuppressContextMenu(facts(el(html)), false, false)).toBe(false);
  });

  it.each([
    ['a code block', '<pre><span data-pick>ls -la</span></pre>'],
    ['inline code', '<p><code data-pick>npm test</code></p>'],
    ['rendered markdown', '<div class="markdown-content"><p data-pick>Hello</p></div>'],
    ['a marked content region', '<div data-native-context-menu><div><span data-pick>step</span></div></div>'],
  ])('keeps the native menu over the text of %s', (_label, html) => {
    expect(shouldSuppressContextMenu(facts(el(html)), false, true)).toBe(false);
  });

  it.each([
    ['the transcript', '<div data-native-context-menu><div data-pick class="gap"></div></div>'],
    ['rendered markdown', '<div class="markdown-content" data-pick></div>'],
  ])('suppresses the page menu over the empty space in %s', (_label, html) => {
    expect(shouldSuppressContextMenu(facts(el(html)), false, false)).toBe(true);
  });

  it('never measures text outside a text region', () => {
    const text = vi.fn(() => true);
    expect(decide(facts(el('<span data-pick>header</span>')), false, text)).toBe(true);
    expect(text).not.toHaveBeenCalled();
  });

  it('suppresses inside contenteditable="false"', () => {
    expect(shouldSuppressContextMenu(facts(el('<div contenteditable="false"><span data-pick>x</span></div>')), false)).toBe(true);
  });

  it('keeps the native menu over a selection the user made before pressing', () => {
    expect(shouldSuppressContextMenu(facts(el('<span data-pick>Settings</span>')), true)).toBe(false);
  });

  it('Option+right-click always shows the native menu', () => {
    expect(shouldSuppressContextMenu(facts(el('<span data-pick>chrome</span>'), { altKey: true }), false)).toBe(false);
  });

  it('leaves a claimed right-click alone', () => {
    expect(shouldSuppressContextMenu(facts(el('<div data-pick>row</div>'), { defaultPrevented: true }), false)).toBe(false);
  });

  it('suppresses over a target with no element behind it', () => {
    expect(shouldSuppressContextMenu(facts(null), false)).toBe(true);
    expect(shouldSuppressContextMenu(facts(document), false)).toBe(true);
  });
});

describe('selectionCovers', () => {
  const rect = { left: 10, right: 60, top: 20, bottom: 36 };
  const selection = (collapsed: boolean) => ({
    isCollapsed: collapsed,
    rangeCount: 1,
    getRangeAt: () => ({ getClientRects: () => [rect] }),
  }) as unknown as Selection;

  it('is true for a point inside a selected rect', () => {
    expect(selectionCovers(selection(false), { x: 30, y: 28 })).toBe(true);
  });

  it('is false for a point outside every selected rect', () => {
    expect(selectionCovers(selection(false), { x: 200, y: 28 })).toBe(false);
  });

  it('is false for a collapsed selection or none', () => {
    expect(selectionCovers(selection(true), { x: 30, y: 28 })).toBe(false);
    expect(selectionCovers(null, { x: 30, y: 28 })).toBe(false);
  });
});

describe('textAt', () => {
  /** A document whose every text node is drawn in one rect at (10..60, 20..36). */
  const doc = {
    createRange: () => ({
      selectNodeContents: () => {},
      getClientRects: () => [{ left: 10, right: 60, top: 20, bottom: 36 }],
    }),
  } as unknown as Document;

  it('is true over the text of the target itself', () => {
    expect(textAt(el('<p data-pick>Hello</p>'), { x: 30, y: 28 }, doc)).toBe(true);
  });

  it('is false beside the text', () => {
    expect(textAt(el('<p data-pick>Hello</p>'), { x: 300, y: 28 }, doc)).toBe(false);
  });

  it('is false for an element holding only whitespace or other elements', () => {
    expect(textAt(el('<div data-pick>   \n </div>'), { x: 30, y: 28 }, doc)).toBe(false);
    expect(textAt(el('<div data-pick><p>Hello</p></div>'), { x: 30, y: 28 }, doc)).toBe(false);
    expect(textAt(null, { x: 30, y: 28 }, doc)).toBe(false);
  });
});

describe('opensContextMenu', () => {
  it('is true for the secondary button and a ctrl-click', () => {
    expect(opensContextMenu({ button: 2, ctrlKey: false })).toBe(true);
    expect(opensContextMenu({ button: 0, ctrlKey: true })).toBe(true);
  });

  it('is false for a plain click', () => {
    expect(opensContextMenu({ button: 0, ctrlKey: false })).toBe(false);
  });
});
