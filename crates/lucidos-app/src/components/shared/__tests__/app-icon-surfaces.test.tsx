// @vitest-environment jsdom
/**
 * Rendered checks that the apps panel row and the menu drawer draw an app's
 * picture through `AppIcon`: the icon file when the app has one, the monogram
 * tile when not (ADR 0414, invariant I3). The source scan beside this file
 * covers every surface; these mount two of them.
 */
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { AppRow } from '../../apps/AppCard';
import { Drawer } from '../../layout/Drawer';
import { drawerOpen } from '../../layout/drawerState';
import { appsList, pinnedApps } from '../../../store/store';
import type { App } from '../../../store/types';

const app = (id: string, name: string, icon?: string): App =>
  ({ id, name, description: '', icon, reveal: 'on-load', kind: 'app', reusable: false });

const WITH_ICON = app('habit-tracker', 'Habit Tracker', 'assets/icon.svg');
const PLAIN = app('demo-director', 'Demo Director');

describe('app surfaces draw AppIcon', () => {
  let host: HTMLDivElement;

  beforeEach(() => {
    host = document.createElement('div');
    document.body.appendChild(host);
  });

  afterEach(() => {
    act(() => {
      render(null, host);
      drawerOpen.value = false;
    });
    host.remove();
  });

  it('the apps panel row leads with the icon file or the monogram tile', () => {
    const noop = () => {};
    render(
      <>
        <AppRow app={WITH_ICON} onOpen={noop} onEdit={noop} onDelete={noop} />
        <AppRow app={PLAIN} onOpen={noop} onEdit={noop} onDelete={noop} />
      </>,
      host,
    );
    const [first, second] = [...host.querySelectorAll('.app-row-lead')];
    expect(first.querySelector('.app-icon-image img')?.getAttribute('src'))
      .toMatch(/\/app\/habit-tracker\/assets\/icon\.svg$/);
    expect(second.querySelector('.app-icon-monogram')?.textContent).toBe('D');
  });

  it('the menu drawer draws pinned apps as tiles and every fixed row a glyph', () => {
    act(() => {
      appsList.value = { status: 'loaded', data: [WITH_ICON, PLAIN] };
      pinnedApps.value = {
        status: 'loaded',
        data: [{ app_id: 'habit-tracker' }, { app_id: 'demo-director' }],
      } as typeof pinnedApps.value;
      drawerOpen.value = true;
    });
    act(() => {
      render(<Drawer />, host);
    });
    const rows = [...document.querySelectorAll<HTMLElement>('.drawer .drawer-item')];
    expect(rows.length).toBeGreaterThan(2);
    expect(rows[0].querySelector('.app-icon-image img')).not.toBeNull();
    expect(rows[1].querySelector('.app-icon-monogram')?.textContent).toBe('D');
    for (const row of rows.slice(2)) {
      expect(row.querySelector('.drawer-item-glyph svg'), row.textContent ?? '').not.toBeNull();
    }
    expect(document.querySelectorAll('.drawer .drawer-menu-divider')).toHaveLength(1);
  });
});
