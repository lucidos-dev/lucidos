// @vitest-environment jsdom
import { describe, it, expect } from 'vitest';
import { shouldOpenFilePreviewMenu } from './filePreviewContextMenuPolicy';

function el(html: string, pick = '[data-pick]'): Element {
  const host = document.createElement('div');
  host.innerHTML = html;
  document.body.appendChild(host);
  return host.querySelector(pick) ?? host.firstElementChild!;
}

const facts = (target: EventTarget | null, over: Partial<{ altKey: boolean; defaultPrevented: boolean }> = {}) =>
  ({ target, altKey: false, defaultPrevented: false, ...over });

describe('shouldOpenFilePreviewMenu', () => {
  it('opens over ordinary, unselected prose', () => {
    expect(shouldOpenFilePreviewMenu(facts(el('<p data-pick>Hello</p>')), false)).toBe(true);
  });

  it('opens over a code line, a diff row, or empty padding alike', () => {
    expect(shouldOpenFilePreviewMenu(facts(el('<pre data-pick>ls -la</pre>')), false)).toBe(true);
    expect(shouldOpenFilePreviewMenu(facts(el('<div class="file-preview-frame-body" data-pick></div>')), false))
      .toBe(true);
  });

  it.each([
    ['a text field', '<input data-pick>'],
    ['a textarea', '<textarea data-pick></textarea>'],
    ['an editable region', '<div contenteditable="true"><p data-pick>x</p></div>'],
    ['a link', '<a href="https://example.com"><span data-pick>docs</span></a>'],
    ['an image', '<img data-pick>'],
    ['a video', '<video data-pick></video>'],
    ['an audio player', '<audio data-pick></audio>'],
  ])('defers to the native menu over %s', (_label, html) => {
    expect(shouldOpenFilePreviewMenu(facts(el(html)), false)).toBe(false);
  });

  it('defers to the native menu over a selection the user made before pressing', () => {
    expect(shouldOpenFilePreviewMenu(facts(el('<p data-pick>Settings</p>')), true)).toBe(false);
  });

  it('Option+right-click always defers to the native menu', () => {
    expect(shouldOpenFilePreviewMenu(facts(el('<p data-pick>chrome</p>'), { altKey: true }), false)).toBe(false);
  });

  it('leaves a claimed right-click alone', () => {
    expect(shouldOpenFilePreviewMenu(facts(el('<div data-pick>row</div>'), { defaultPrevented: true }), false))
      .toBe(false);
  });

  it('defers for a target with no element behind it', () => {
    expect(shouldOpenFilePreviewMenu(facts(null), false)).toBe(false);
    expect(shouldOpenFilePreviewMenu(facts(document), false)).toBe(false);
  });
});
