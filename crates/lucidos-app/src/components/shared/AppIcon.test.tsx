// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { AppIcon, ListedAppIcon, MONOGRAM_HUES, monogramHue, monogramLetter } from './AppIcon';
import { appsList } from '../../store/store';
import type { App } from '../../store/types';

describe('the monogram tile', () => {
  it('picks the same theme hue for the same id, every time (I4)', () => {
    const ids = ['habit-tracker', 'demo-director', 'weather-now', 'a', ''];
    for (const id of ids) {
      expect(monogramHue(id)).toBe(monogramHue(id));
      expect(MONOGRAM_HUES).toContain(monogramHue(id));
    }
  });

  it('spreads ids over more than one hue', () => {
    const hues = new Set(['a', 'b', 'c', 'd', 'e', 'f', 'g', 'h'].map(monogramHue));
    expect(hues.size).toBeGreaterThan(1);
  });

  it('offers theme variables only', () => {
    for (const hue of MONOGRAM_HUES) expect(hue).toMatch(/^--[a-z-]+$/);
  });

  it('letters the name, whole characters included', () => {
    expect(monogramLetter('habit tracker')).toBe('H');
    expect(monogramLetter('  spaced')).toBe('S');
    expect(monogramLetter('\u{1F331} garden')).toBe('\u{1F331}');
    expect(monogramLetter('')).toBe('?');
  });
});

describe('AppIcon', () => {
  let host: HTMLDivElement;

  beforeEach(() => {
    host = document.createElement('div');
    document.body.appendChild(host);
  });

  afterEach(() => {
    render(null, host);
    host.remove();
  });

  const tile = () => host.querySelector<HTMLElement>('.app-icon')!;

  it('draws the icon file as an image on a tile', () => {
    render(<AppIcon appId="habit-tracker" name="Habit Tracker" icon="assets/icon.svg" />, host);
    expect(tile().classList).toContain('app-icon-image');
    const img = host.querySelector('img')!;
    expect(img.getAttribute('src')).toMatch(/\/app\/habit-tracker\/assets\/icon\.svg$/);
    expect(img.getAttribute('alt')).toBe('');
  });

  it('draws the monogram tile without an icon, in its theme hue', () => {
    render(<AppIcon appId="habit-tracker" name="Habit Tracker" />, host);
    expect(tile().classList).toContain('app-icon-monogram');
    expect(tile().textContent).toBe('H');
    expect(tile().style.getPropertyValue('--app-icon-hue')).toBe(`var(${monogramHue('habit-tracker')})`);
    expect(host.querySelector('img')).toBeNull();
  });

  it('falls back to the monogram when the image fails to load (I5)', () => {
    render(<AppIcon appId="habit-tracker" name="Habit Tracker" icon="assets/gone.svg" />, host);
    act(() => {
      host.querySelector('img')!.dispatchEvent(new Event('error'));
    });
    expect(host.querySelector('img')).toBeNull();
    expect(tile().classList).toContain('app-icon-monogram');
  });

  it('tries again when the icon changes after a failure', () => {
    render(<AppIcon appId="habit-tracker" name="Habit Tracker" icon="assets/gone.svg" />, host);
    act(() => {
      host.querySelector('img')!.dispatchEvent(new Event('error'));
    });
    render(<AppIcon appId="habit-tracker" name="Habit Tracker" icon="assets/new.svg" />, host);
    expect(host.querySelector('img')?.getAttribute('src')).toMatch(/assets\/new\.svg$/);
  });

  it('encodes each path segment, keeping the slashes', () => {
    render(<AppIcon appId="habit-tracker" name="H" icon="my assets/icon #1.png" />, host);
    expect(host.querySelector('img')!.getAttribute('src'))
      .toMatch(/\/app\/habit-tracker\/my%20assets\/icon%20%231\.png$/);
  });

  it('reads the icon from the apps list when it knows only the id', () => {
    appsList.value = { status: 'loading' };
    render(<ListedAppIcon appId="habit-tracker" name="Habit Tracker" />, host);
    expect(tile().classList).toContain('app-icon-monogram');
    act(() => {
      const app = { id: 'habit-tracker', name: 'Habit Tracker', icon: 'assets/icon.svg' } as App;
      appsList.value = { status: 'loaded', data: [app] };
    });
    expect(tile().classList).toContain('app-icon-image');
  });
});

describe('AppIcon after a failed load', () => {
  let host: HTMLDivElement;

  beforeEach(() => {
    host = document.createElement('div');
    document.body.appendChild(host);
  });

  afterEach(() => {
    render(null, host);
    host.remove();
  });

  it('tries the same path again once the apps list is read again', () => {
    appsList.value = { status: 'loaded', data: [] };
    render(<AppIcon appId="habit-tracker" name="Habit Tracker" icon="assets/icon.svg" />, host);
    act(() => {
      host.querySelector('img')!.dispatchEvent(new Event('error'));
    });
    expect(host.querySelector('img')).toBeNull();
    act(() => {
      appsList.value = { status: 'loaded', data: [] };
    });
    expect(host.querySelector('img')?.getAttribute('src')).toMatch(/assets\/icon\.svg$/);
  });
});
