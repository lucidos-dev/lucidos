// @vitest-environment jsdom
/** Settings › Appearance lists every theme inline, in a carousel filtered by
 *  family. A pick paints at once, and nothing folds or swallows the next tap. */
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { DEFAULT_THEME_ID, FALLBACK_FONT, FONT_STACKS, THEME_KEY, fontFeaturesFor, type WorkspaceFont } from '@lucidos/appearance';
import type { Theme, ThemeFamily } from '../../../api/client';
import { themeGallery, pickTheme as pickThemeAction } from '../../../store/actions/themes';

vi.mock('../../../store/actions/themes', async importOriginal => ({
  ...(await importOriginal<typeof import('../../../store/actions/themes')>()),
  pickTheme: vi.fn(),
}));
import { paintedThemeMode } from '../../../store/actions/preferences';
import { preferences } from '../../../store/store';
import { viewportIsMobile } from '../../../utils/viewport';
import { motionPreference } from '../../../utils/motion';
import { ThemePicker, groupThemesByFamily } from '../ThemePicker';

function theme(id: string, name: string, family?: ThemeFamily): Theme {
  return { id, source: 'built-in', name, family, modes: ['dark', 'light'], resolved: { dark: {}, light: {}, fonts: {}, workspace_fonts: [] } };
}

function workspaceTheme(id: string, name: string, family?: ThemeFamily): Theme {
  return { ...theme(id, name, family), source: 'workspace' };
}

let host: HTMLDivElement;

beforeEach(() => {
  themeGallery.value = {
    status: 'loaded',
    data: {
      themes: [theme(DEFAULT_THEME_ID, 'Lucidos', 'blue'), theme('paper', 'Paper', 'warm'), theme('nord', 'Nord', 'blue')],
      defaults: { dark: {}, light: {} },
    },
  };
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
  themeGallery.value = { status: 'not-loaded' };
  preferences.value = { status: 'not-loaded' };
  vi.mocked(pickThemeAction).mockClear();
  document.documentElement.style.fontSize = '';
});

function showPicker() {
  act(() => { render(<ThemePicker />, host); });
}

function pickTheme(id: string) {
  act(() => { preferences.value = { status: 'loaded', data: { [THEME_KEY]: id } }; });
}

function radios(): HTMLButtonElement[] {
  return [...host.querySelectorAll<HTMLButtonElement>('.theme-card[role="radio"]')];
}

function checked(): HTMLButtonElement | undefined {
  return radios().find(card => card.getAttribute('aria-checked') === 'true');
}

function card(name: string): HTMLButtonElement | undefined {
  return radios().find(radio => radio.textContent?.includes(name));
}

it('lists every theme inline as a card, the active one checked, with no toggle', () => {
  showPicker();
  expect(radios()).toHaveLength(3);
  expect(checked()?.textContent).toContain('Lucidos');
  expect(host.querySelector('[aria-expanded]')).toBeNull();
});

it('places each family on fresh columns, two cards tall, named above its first card', () => {
  showPicker();
  const strip = host.querySelector('.theme-carousel');
  expect(strip?.getAttribute('role')).toBe('radiogroup');
  const cell = (name: string) => {
    const style = card(name)!.style;
    return [style.gridColumn, style.gridRow];
  };
  expect(cell('Lucidos')).toEqual(['1', '2']);
  expect(cell('Nord')).toEqual(['1', '3']);
  expect(cell('Paper')).toEqual(['2', '2']);
  const names = [...host.querySelectorAll<HTMLElement>('.theme-family-name')];
  expect(names.map(name => [name.textContent, name.style.gridColumn])).toEqual([['Cool', '1 / span 1'], ['Warm', '2 / span 1']]);
});

it('draws one named group per family, with similar themes side by side', () => {
  showPicker();
  const sections = [...host.querySelectorAll('.theme-family')].map(section => ({
    name: section.getAttribute('aria-label'),
    themes: [...section.querySelectorAll('.theme-card-name')].map(name => name.textContent),
  }));
  expect(sections).toEqual([
    { name: 'Cool', themes: ['Lucidos', 'Nord'] },
    { name: 'Warm', themes: ['Paper'] },
  ]);
  expect(host.querySelectorAll('[role="radiogroup"]')).toHaveLength(1);
});

function chip(name: string): HTMLButtonElement {
  const button = [...host.querySelectorAll<HTMLButtonElement>('.pill-bar-btn')].find(c => c.textContent === name);
  if (!button) throw new Error(`no ${name} chip`);
  return button;
}

it('filters by family: All first and picked, then one chip per family', () => {
  showPicker();
  expect([...host.querySelectorAll('.pill-bar-btn')].map(c => c.textContent)).toEqual(['All', 'Cool', 'Warm']);
  expect(chip('All').getAttribute('aria-pressed')).toBe('true');
  act(() => { chip('Warm').click(); });
  expect(chip('Warm').getAttribute('aria-pressed')).toBe('true');
  expect(chip('All').getAttribute('aria-pressed')).toBe('false');
  expect(radios().map(r => r.querySelector('.theme-card-name')?.textContent)).toEqual(['Paper']);
  // The chip names the family, so the strip drops its names row.
  expect(host.querySelector('.theme-family-name')).toBeNull();
  expect(card('Paper')!.style.gridRow).toBe('1');
  act(() => { chip('All').click(); });
  expect(radios()).toHaveLength(3);
});

/** Gives the strip a height layout would: taller while it draws the names row.
 *  Records each animation the strip itself runs. */
function measureStripHeight(): () => Keyframe[][] {
  const strip = host.querySelector<HTMLElement>('.theme-carousel')!;
  strip.getBoundingClientRect = () => ({ height: host.querySelector('.theme-family-name') ? 200 : 100 }) as DOMRect;
  const played: { target: Element; frames: Keyframe[] }[] = [];
  HTMLElement.prototype.animate = vi.fn(function (this: HTMLElement, frames: Keyframe[]) {
    played.push({ target: this, frames });
    return { cancel: vi.fn(), finished: new Promise(() => {}) } as unknown as Animation;
  }) as unknown as typeof HTMLElement.prototype.animate;
  return () => played.filter(p => p.target === strip).map(p => p.frames);
}

afterEach(() => {
  delete (HTMLElement.prototype as { animate?: unknown }).animate;
});

it('eases the strip to its new height when a family chip changes it', () => {
  showPicker();
  const animations = measureStripHeight();
  act(() => { chip('Warm').click(); });
  act(() => { chip('All').click(); });
  expect(animations()).toEqual([
    [{ height: '200px' }, { height: '100px' }],
    [{ height: '100px' }, { height: '200px' }],
  ]);
});

it('snaps the strip to its new height under reduced motion', () => {
  act(() => { motionPreference.value = 'reduce'; });
  try {
    showPicker();
    const animations = measureStripHeight();
    act(() => { chip('Warm').click(); });
    expect(animations()).toEqual([]);
  } finally {
    act(() => { motionPreference.value = 'system'; });
  }
});

function step(label: string): HTMLButtonElement {
  const button = host.querySelector<HTMLButtonElement>(`.theme-carousel-step[aria-label="${label}"]`);
  if (!button) throw new Error(`no ${label} chevron`);
  return button;
}

/** Gives the strip the box layout would: 300px wide, no padding or gap, on a
 *  16px root, with `scrollWidth` of content. jsdom lays nothing out. */
function sizeStrip(scrollLeft: number, scrollWidth = 900) {
  document.documentElement.style.fontSize = '16px';
  const strip = host.querySelector<HTMLElement>('.theme-carousel')!;
  strip.style.padding = '0px';
  strip.style.columnGap = '0px';
  Object.defineProperty(strip, 'clientWidth', { configurable: true, value: 300 });
  Object.defineProperty(strip, 'scrollWidth', { configurable: true, value: scrollWidth });
  strip.scrollLeft = scrollLeft;
  act(() => { strip.dispatchEvent(new Event('scroll')); });
  return strip;
}

function recordScrolls(strip: HTMLElement): () => (number | undefined)[] {
  const scrollTo = vi.fn();
  strip.scrollTo = scrollTo as typeof strip.scrollTo;
  return () => scrollTo.mock.calls.map(([options]) => (options as ScrollToOptions).left);
}

it('draws a chevron on each side, both off while every theme fits', () => {
  showPicker();
  expect(step('Previous themes').disabled).toBe(true);
  expect(step('Next themes').disabled).toBe(true);
});

it('turns each chevron on while there is more strip on its side', () => {
  showPicker();
  sizeStrip(0);
  expect(step('Previous themes').disabled).toBe(true);
  expect(step('Next themes').disabled).toBe(false);
  sizeStrip(300);
  expect(step('Previous themes').disabled).toBe(false);
  expect(step('Next themes').disabled).toBe(false);
  sizeStrip(600);
  expect(step('Next themes').disabled).toBe(true);
});

it('pages by whole columns: a page is every column that fits', () => {
  showPicker();
  // 300px holds two 9rem (144px) columns, so a page is the full 300px.
  const strip = sizeStrip(300);
  const scrolls = recordScrolls(strip);
  act(() => { step('Next themes').click(); });
  act(() => { step('Previous themes').click(); });
  expect(scrolls()).toEqual([600, 0]);
});

it('drops to one row when every card fits in it, and back to two when not', () => {
  let resized: () => void = () => {};
  vi.stubGlobal('ResizeObserver', class {
    constructor(callback: () => void) { resized = callback; }
    observe() {}
    disconnect() {}
  });
  try {
    showPicker();
    const strip = sizeStrip(0);
    const cells = () => ['Lucidos', 'Nord', 'Paper'].map(name => [card(name)!.style.gridColumn, card(name)!.style.gridRow]);
    // 300px holds two 9rem (144px) columns, too few for three cards.
    act(() => { resized(); });
    expect(cells()).toEqual([['1', '2'], ['1', '3'], ['2', '2']]);
    // 600px holds four, so all three sit on the row under the family names.
    Object.defineProperty(strip, 'clientWidth', { configurable: true, value: 600 });
    act(() => { resized(); });
    expect(cells()).toEqual([['1', '2'], ['2', '2'], ['3', '2']]);
  } finally {
    vi.unstubAllGlobals();
  }
});

it('never skips a column when the strip rests between page stops', () => {
  showPicker();
  // Clamped at the end: 700px of strip scrolls at most 400px, a third of the
  // way past the 300px stop. Back must land on that stop, not skip to 0.
  let strip = sizeStrip(400, 700);
  let scrolls = recordScrolls(strip);
  act(() => { step('Previous themes').click(); });
  expect(scrolls()).toEqual([300]);
  // A swipe left it half a page in: on is the next stop, not the one after.
  strip = sizeStrip(150);
  scrolls = recordScrolls(strip);
  act(() => { step('Next themes').click(); });
  expect(scrolls()).toEqual([300]);
});

it('on a phone, pages one card at a time, the first press centring the third', () => {
  act(() => { viewportIsMobile.value = true; });
  try {
    showPicker();
    const strip = sizeStrip(0);
    expect(strip.classList.contains('theme-carousel-peek')).toBe(true);
    // One row, and no names row above it.
    expect(card('Nord')!.style.gridRow).toBe('2');
    expect(card('Nord')!.style.gridColumn).toBe('2');
    const scrolls = recordScrolls(strip);
    act(() => { step('Next themes').click(); });
    // A 7rem (112px) card centred in 300px stops 94px short of its column.
    // At the start the second card is nearest the centre, so the press
    // centres the third: 2 * 112 - 94.
    expect(scrolls()).toEqual([130]);
  } finally {
    act(() => { viewportIsMobile.value = false; });
  }
});

it('orders families as the engine does and puts themes with no family last, as Other', () => {
  const groups = groupThemesByFamily([
    theme('mine', 'Mine'),
    theme('mono', 'Mono', 'neutral'),
    theme('amethyst', 'Amethyst', 'violet'),
    theme('gruvbox', 'Gruvbox', 'warm'),
    theme('nord', 'Nord', 'blue'),
  ]);
  expect(groups.map(group => [group.name, group.themes.map(l => l.id)])).toEqual([
    ['Cool', ['nord']],
    ['Violet', ['amethyst']],
    ['Warm', ['gruvbox']],
    ['Neutral', ['mono']],
    ['Other', ['mine']],
  ]);
});

it('puts a family this bundle does not know under Other rather than dropping the theme', () => {
  const future = { ...theme('future', 'Future'), family: 'teal' as unknown as ThemeFamily };
  expect(groupThemesByFamily([future]).map(group => [group.name, group.themes.map(l => l.id)]))
    .toEqual([['Other', ['future']]]);
});

it('picks each theme tapped and keeps every card up, so themes can be compared', async () => {
  showPicker();
  await act(async () => { card('Nord')?.click(); });
  await act(async () => { card('Paper')?.click(); });
  expect(vi.mocked(pickThemeAction).mock.calls.map(([picked]) => picked.id)).toEqual(['nord', 'paper']);
  expect(radios()).toHaveLength(3);
});

it('does nothing on a tap on the active theme, which needs no pick', () => {
  showPicker();
  act(() => { checked()?.click(); });
  expect(vi.mocked(pickThemeAction)).not.toHaveBeenCalled();
  expect(radios()).toHaveLength(3);
});

it('still offers the mode switch when the active theme has no mode for this device', () => {
  act(() => { paintedThemeMode.value = 'dark'; });
  const paper = { ...theme('paper', 'Paper', 'warm'), modes: ['light' as const] };
  themeGallery.value = { status: 'loaded', data: { themes: [theme(DEFAULT_THEME_ID, 'Lucidos', 'blue'), paper], defaults: { dark: {}, light: {} } } };
  pickTheme('paper');
  showPicker();
  act(() => { checked()?.click(); });
  expect(vi.mocked(pickThemeAction)).toHaveBeenCalledWith(expect.objectContaining({ id: 'paper' }));
});

it('lets the next tap elsewhere through, since nothing is left to dismiss', async () => {
  const pressed = vi.fn();
  const control = document.createElement('button');
  control.addEventListener('click', pressed);
  document.body.appendChild(control);
  showPicker();
  await act(async () => { card('Nord')?.click(); });
  act(() => { control.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true, detail: 1 })); });
  expect(pressed).toHaveBeenCalledOnce();
  expect(radios()).toHaveLength(3);
  control.remove();
});

it('previews and checks the theme the preference picks', () => {
  pickTheme('nord');
  showPicker();
  expect(checked()?.textContent).toContain('Nord');
});

it('previews and checks the default when the picked theme is gone, since that is what paints', () => {
  pickTheme('removed-plugin-theme');
  showPicker();
  expect(radios().filter(card => card.getAttribute('aria-checked') === 'true')).toHaveLength(1);
  expect(checked()?.textContent).toContain('Lucidos');
});

it('badges a workspace theme as Custom, and leaves a built-in theme unbadged', () => {
  themeGallery.value = {
    status: 'loaded',
    data: {
      themes: [theme(DEFAULT_THEME_ID, 'Lucidos', 'blue'), workspaceTheme('mine', 'Mine', 'blue')],
      defaults: { dark: {}, light: {} },
    },
  };
  showPicker();
  expect(card('Lucidos')?.querySelector('.theme-card-badge')).toBeNull();
  expect(card('Mine')?.querySelector('.theme-card-badge')?.textContent).toBe('Custom');
});

it("names each theme in the font that theme suggests, not the page's", () => {
  themeGallery.value = {
    status: 'loaded',
    data: {
      themes: [
        { ...theme('geist-theme', 'Geist Theme', 'blue'), resolved: { dark: {}, light: {}, fonts: { ui: 'geist' }, workspace_fonts: [] } },
        theme('plain', 'Plain', 'blue'),
        theme(DEFAULT_THEME_ID, 'Lucidos', 'blue'),
      ],
      defaults: { dark: {}, light: {} },
    },
  };
  showPicker();
  const metas = [...host.querySelectorAll<HTMLElement>('.theme-card-meta')];
  const font = (meta: HTMLElement, prop: string) => meta.style.getPropertyValue(prop);
  expect(font(metas[0], '--font-ui')).toBe(FONT_STACKS.geist);
  expect(font(metas[0], '--font-features-text')).toBe(fontFeaturesFor('geist').text);
  // A theme that suggests no font names itself in the fallback, as it would paint.
  expect(font(metas[1], '--font-ui')).toBe(FONT_STACKS[FALLBACK_FONT]);
  expect(font(metas[1], '--font-features-text')).toBe(fontFeaturesFor(FALLBACK_FONT).text);
});

it('names a card in the workspace font its theme suggests, from the entry the theme carries', () => {
  const brand: WorkspaceFont = {
    id: 'ws-brand',
    label: 'Brand',
    family: 'ws-brand',
    stack: "'ws-brand', system-ui, sans-serif",
    group: 'sans',
    ligatures: false,
    faces: [{ path: 'fonts/brand/a.woff2', weight: '400', style: 'normal' }],
  };
  themeGallery.value = {
    status: 'loaded',
    data: {
      themes: [{ ...theme(DEFAULT_THEME_ID, 'Branded', 'blue'), resolved: { dark: {}, light: {}, fonts: { ui: 'ws-brand' }, workspace_fonts: [brand] } }],
      defaults: { dark: {}, light: {} },
    },
  };
  showPicker();
  const meta = checked()?.querySelector<HTMLElement>('.theme-card-meta');
  expect(meta?.style.getPropertyValue('--font-ui')).toBe(brand.stack);
});

it('holds its box while the gallery loads, before the skeleton is due', () => {
  themeGallery.value = { status: 'loading' };
  showPicker();
  // No loader yet: the delay gate has not fired.
  expect(host.querySelector('.loading-fade-skeleton')).toBeNull();
  // The skeleton's own frame, drawn invisibly, so the rows below never jump.
  const reserve = host.querySelector<HTMLElement>('.loading-fade-content .theme-carousel-reserve');
  expect(reserve?.getAttribute('aria-hidden')).toBe('true');
  expect(reserve?.querySelector('.pill-bar')).not.toBeNull();
  expect(reserve?.querySelectorAll('.theme-carousel .theme-card')).toHaveLength(6);
  expect(radios().filter(card => !card.closest('.theme-carousel-reserve'))).toHaveLength(0);
});
