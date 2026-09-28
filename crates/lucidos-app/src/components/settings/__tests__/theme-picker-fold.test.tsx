// @vitest-environment jsdom
/** Settings › Appearance › Theme opens on the active theme's preview card alone.
 *  The grid of previews stays folded until the reader asks for it. */
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
import { ThemePicker, groupThemesByFamily } from '../ThemePicker';

function theme(id: string, name: string, family?: ThemeFamily): Theme {
  return { id, source: 'built-in', name, family, modes: ['dark', 'light'], resolved: { dark: {}, light: {}, fonts: {}, workspace_fonts: [] } };
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
  act(() => { render(<ThemePicker />, host); });
});

afterEach(() => {
  render(null, host);
  host.remove();
  themeGallery.value = { status: 'not-loaded' };
  preferences.value = { status: 'not-loaded' };
  vi.mocked(pickThemeAction).mockClear();
});

function pickTheme(id: string) {
  act(() => { preferences.value = { status: 'loaded', data: { [THEME_KEY]: id } }; });
}

function toggle(): HTMLButtonElement {
  const button = host.querySelector<HTMLButtonElement>('.theme-toggle');
  if (!button) throw new Error('no theme toggle rendered');
  return button;
}

function radios(): HTMLButtonElement[] {
  return [...host.querySelectorAll<HTMLButtonElement>('.theme-card[role="radio"]')];
}

function checked(): HTMLButtonElement | undefined {
  return radios().find(card => card.getAttribute('aria-checked') === 'true');
}

it('opens shut, showing only the active theme as a preview card and how many there are', () => {
  expect(toggle().getAttribute('aria-expanded')).toBe('false');
  expect(toggle().classList.contains('theme-card')).toBe(true);
  expect(toggle().querySelector('.theme-preview')).not.toBeNull();
  expect(toggle().querySelector('.theme-card-name')?.textContent).toBe('Lucidos');
  expect(toggle().querySelector('.theme-toggle-count')?.textContent).toBe('3 themes');
  expect(toggle().querySelector('.theme-toggle-chevron svg')).not.toBeNull();
  expect(host.querySelectorAll('.theme-card')).toHaveLength(1);
  expect(radios()).toHaveLength(0);
});

it('unfolds every theme as a card, the active one checked', () => {
  act(() => { toggle().click(); });
  expect(radios()).toHaveLength(3);
  expect(checked()?.textContent).toContain('Lucidos');
});

it('draws one named section per family, with similar themes side by side', () => {
  act(() => { toggle().click(); });
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

it('folds the grid away again on a tap on the active theme', () => {
  act(() => { toggle().click(); });
  act(() => { checked()?.click(); });
  expect(radios()).toHaveLength(0);
  expect(toggle().getAttribute('aria-expanded')).toBe('false');
});

it('keeps the grid open when another theme is picked, so themes can be compared', async () => {
  act(() => { toggle().click(); });
  await act(async () => { card('Nord')?.click(); });
  expect(vi.mocked(pickThemeAction)).toHaveBeenCalledWith(expect.objectContaining({ id: 'nord' }));
  expect(radios()).toHaveLength(3);
});

it('still offers the mode switch when the active theme has no mode for this device', () => {
  act(() => { paintedThemeMode.value = 'dark'; });
  const paper = { ...theme('paper', 'Paper', 'warm'), modes: ['light' as const] };
  act(() => {
    themeGallery.value = { status: 'loaded', data: { themes: [theme(DEFAULT_THEME_ID, 'Lucidos', 'blue'), paper], defaults: { dark: {}, light: {} } } };
  });
  pickTheme('paper');
  act(() => { toggle().click(); });
  act(() => { checked()?.click(); });
  expect(vi.mocked(pickThemeAction)).toHaveBeenCalledWith(expect.objectContaining({ id: 'paper' }));
  expect(radios()).toHaveLength(2);
});

function card(name: string): HTMLButtonElement | undefined {
  return radios().find(radio => radio.textContent?.includes(name));
}

it('folds on a click elsewhere, with no pick first', () => {
  act(() => { toggle().click(); });
  act(() => { document.body.click(); });
  expect(radios()).toHaveLength(0);
  expect(toggle().getAttribute('aria-expanded')).toBe('false');
});

it('stays open while themes are picked, then folds on a click elsewhere', async () => {
  act(() => { toggle().click(); });
  await act(async () => { card('Nord')?.click(); });
  await act(async () => { card('Paper')?.click(); });
  expect(radios()).toHaveLength(3);
  act(() => { document.body.click(); });
  expect(radios()).toHaveLength(0);
});

it('stays open on a click that answers an overlay, such as the mode-switch confirm', () => {
  act(() => { toggle().click(); });
  document.documentElement.setAttribute('data-overlay-open', '');
  act(() => { document.body.click(); });
  document.documentElement.removeAttribute('data-overlay-open');
  expect(radios()).toHaveLength(3);
});

it('stays open on the tap that dismisses an overlay, which closes before the tap lifts', () => {
  act(() => { toggle().click(); });
  document.documentElement.setAttribute('data-overlay-open', '');
  document.body.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, isPrimary: true }));
  document.documentElement.removeAttribute('data-overlay-open');
  mouseClick(document.body);
  expect(radios()).toHaveLength(3);
  act(() => { document.body.click(); });
  expect(radios()).toHaveLength(0);
});

it('folds on a click whose control stops its propagation', () => {
  const control = document.createElement('button');
  control.addEventListener('click', e => e.stopPropagation());
  document.body.appendChild(control);
  act(() => { toggle().click(); });
  mouseClick(control);
  expect(radios()).toHaveLength(0);
  control.remove();
});

/** A mouse click, which counts one press in `detail`. A bare `.click()` is
 *  what a keyboard press dispatches, with `detail` 0. */
function mouseClick(target: Element): void {
  act(() => { target.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true, detail: 1 })); });
}

it('swallows the click that folds it, so the control under it does not fire', () => {
  const pressed = vi.fn();
  const control = document.createElement('button');
  control.addEventListener('click', pressed);
  document.body.appendChild(control);
  act(() => { toggle().click(); });
  mouseClick(control);
  expect(radios()).toHaveLength(0);
  expect(pressed).not.toHaveBeenCalled();
  mouseClick(control);
  expect(pressed).toHaveBeenCalledOnce();
  control.remove();
});

it('folds on a keyboard press elsewhere and still presses that control', () => {
  const pressed = vi.fn();
  const control = document.createElement('button');
  control.addEventListener('click', pressed);
  document.body.appendChild(control);
  act(() => { toggle().click(); });
  act(() => { control.click(); });
  expect(radios()).toHaveLength(0);
  expect(pressed).toHaveBeenCalledOnce();
  control.remove();
});

function touchTap(target: Element, moveY = 0): Event {
  target.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, isPrimary: true, screenX: 10, screenY: 10 }));
  if (moveY) target.dispatchEvent(new PointerEvent('pointermove', { bubbles: true, isPrimary: true, screenX: 10, screenY: 10 + moveY }));
  const end = new Event('touchend', { bubbles: true, cancelable: true });
  act(() => { target.dispatchEvent(end); });
  return end;
}

it('swallows a touch tap that folds it, before a control that acts on touchend', () => {
  const pressed = vi.fn();
  const control = document.createElement('button');
  control.addEventListener('touchend', pressed);
  document.body.appendChild(control);
  act(() => { toggle().click(); });
  const end = touchTap(control);
  expect(radios()).toHaveLength(0);
  expect(pressed).not.toHaveBeenCalled();
  expect(end.defaultPrevented).toBe(true);
  control.remove();
});

it('stays open, swallowing nothing, when a touch scrolls the page', () => {
  const pressed = vi.fn();
  const control = document.createElement('button');
  control.addEventListener('touchend', pressed);
  document.body.appendChild(control);
  act(() => { toggle().click(); });
  const end = touchTap(control, 40);
  expect(radios()).toHaveLength(3);
  expect(pressed).toHaveBeenCalledOnce();
  expect(end.defaultPrevented).toBe(false);
  control.remove();
});

it('stays open when the system takes the touch for a scroll', () => {
  const control = document.createElement('button');
  document.body.appendChild(control);
  act(() => { toggle().click(); });
  control.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, isPrimary: true, screenX: 10, screenY: 10 }));
  control.dispatchEvent(new PointerEvent('pointercancel', { bubbles: true, isPrimary: true }));
  const end = new Event('touchend', { bubbles: true, cancelable: true });
  act(() => { control.dispatchEvent(end); });
  expect(radios()).toHaveLength(3);
  expect(end.defaultPrevented).toBe(false);
  control.remove();
});

it('ignores the click iOS sends after a touch that scrolled', () => {
  const pressed = vi.fn();
  const control = document.createElement('button');
  control.addEventListener('click', pressed);
  document.body.appendChild(control);
  act(() => { toggle().click(); });
  touchTap(control, 9);
  mouseClick(control);
  expect(radios()).toHaveLength(3);
  expect(pressed).toHaveBeenCalledOnce();
  control.remove();
});

it('folds on a toast tap and still presses the toast, as an overlay does', () => {
  const pressed = vi.fn();
  const toasts = document.createElement('div');
  toasts.className = 'toast-container';
  const dismiss = document.createElement('button');
  dismiss.addEventListener('click', pressed);
  toasts.appendChild(dismiss);
  document.body.appendChild(toasts);
  act(() => { toggle().click(); });
  mouseClick(dismiss);
  expect(radios()).toHaveLength(0);
  expect(pressed).toHaveBeenCalledOnce();
  toasts.remove();
});

it('reads a tap by its first finger, whatever a second one does', () => {
  const control = document.createElement('button');
  document.body.appendChild(control);
  act(() => { toggle().click(); });
  control.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, isPrimary: true, screenX: 10, screenY: 10 }));
  control.dispatchEvent(new PointerEvent('pointermove', { bubbles: true, isPrimary: false, screenX: 200, screenY: 200 }));
  const end = new Event('touchend', { bubbles: true, cancelable: true });
  act(() => { control.dispatchEvent(end); });
  expect(radios()).toHaveLength(0);
  expect(end.defaultPrevented).toBe(true);
  control.remove();
});

it('lets a touch tap on another theme through, to pick it', () => {
  act(() => { toggle().click(); });
  const nord = card('Nord');
  if (!nord) throw new Error('no Nord card');
  const end = touchTap(nord);
  expect(end.defaultPrevented).toBe(false);
  expect(radios()).toHaveLength(3);
});

it('previews and checks the theme the preference picks', () => {
  pickTheme('nord');
  expect(toggle().querySelector('.theme-card-name')?.textContent).toBe('Nord');
  act(() => { toggle().click(); });
  expect(checked()?.textContent).toContain('Nord');
});

it('previews and checks the default when the picked theme is gone, since that is what paints', () => {
  pickTheme('removed-plugin-theme');
  expect(toggle().querySelector('.theme-card-name')?.textContent).toBe('Lucidos');
  act(() => { toggle().click(); });
  expect(radios().filter(card => card.getAttribute('aria-checked') === 'true')).toHaveLength(1);
  expect(checked()?.textContent).toContain('Lucidos');
});

it('draws no summary card until the themes load', () => {
  act(() => { themeGallery.value = { status: 'loading' }; });
  expect(host.querySelector('.theme-toggle')).toBeNull();
});

it("names each theme in the font that theme suggests, not the page's", () => {
  act(() => {
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
  });
  act(() => { toggle().click(); });
  const metas = [...host.querySelectorAll<HTMLElement>('.theme-card-meta')];
  const font = (meta: HTMLElement, prop: string) => meta.style.getPropertyValue(prop);
  expect(font(metas[0], '--font-ui')).toBe(FONT_STACKS.geist);
  expect(font(metas[0], '--font-features-text')).toBe(fontFeaturesFor('geist').text);
  // A theme that suggests no font names itself in the fallback, as it would paint.
  expect(font(metas[1], '--font-ui')).toBe(FONT_STACKS[FALLBACK_FONT]);
  expect(font(metas[1], '--font-features-text')).toBe(fontFeaturesFor(FALLBACK_FONT).text);
});

it('names the folded active theme in the workspace font it suggests, from the entry the theme carries', () => {
  const brand: WorkspaceFont = {
    id: 'ws-brand',
    label: 'Brand',
    family: 'ws-brand',
    stack: "'ws-brand', system-ui, sans-serif",
    group: 'sans',
    ligatures: false,
    faces: [{ path: 'fonts/brand/a.woff2', weight: '400', style: 'normal' }],
  };
  act(() => {
    themeGallery.value = {
      status: 'loaded',
      data: {
        themes: [{ ...theme(DEFAULT_THEME_ID, 'Branded', 'blue'), resolved: { dark: {}, light: {}, fonts: { ui: 'ws-brand' }, workspace_fonts: [brand] } }],
        defaults: { dark: {}, light: {} },
      },
    };
  });
  const meta = toggle().querySelector<HTMLElement>('.theme-card-meta');
  expect(meta?.style.getPropertyValue('--font-ui')).toBe(brand.stack);
});
