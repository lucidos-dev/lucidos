import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
import type { VNode } from 'preact';
import { CategoryIcon } from '../CategoryIcon';
import { AppIcon } from '../AppIcon';
import { AppsIcon } from '../icons';
import { resolveAppInfo } from '../../chat/MessageRoutePanel';
import { appsList } from '../../../store/store';

const source = (rel: string): string => readFileSync(new URL(rel, import.meta.url), 'utf8');

const APP_SURFACES = [
  '../../apps/AppCard.tsx',
  '../../apps/ReusableWidgetsSection.tsx',
  '../../dashboard/DashboardTile.tsx',
  '../../plugins/pluginCard.tsx',
  '../../layout/Drawer.tsx',
  '../../widgets/WidgetPickerItems.tsx',
  '../../search/SearchEverywhere.tsx',
  '../../layout/ContentNav.tsx',
  '../../chat/MessageRoutePanel.tsx',
];

const ALLOWED_ICON_READS = [
  /icon=\{[\w$?.]+\.icon\}/,
  /appIcon: [\w$?.]+\.icon\b/,
  /\{appInfo\.icon\}/,
];

describe('every app surface draws its picture through AppIcon (I3)', () => {
  for (const rel of APP_SURFACES) {
    const src = source(rel);
    it(`${rel} renders AppIcon`, () => {
      expect(src).toMatch(/<(AppIcon|ListedAppIcon)\b/);
    });

    it(`${rel} draws no generic glyph for an app or widget`, () => {
      expect(src).not.toMatch(/<AppsIcon\b/);
    });

    it(`${rel} reads an icon field only to hand it to AppIcon`, () => {
      const reads = src.split('\n').filter((line) => /\.icon\b(?!-)/.test(line));
      const stray = reads.filter((line) => !ALLOWED_ICON_READS.some((ok) => ok.test(line)));
      expect(stray).toEqual([]);
    });
  }

  it('only AppIcon builds an app file URL for a picture', () => {
    for (const rel of APP_SURFACES) expect(source(rel)).not.toContain('appFileUrl(');
  });
});

describe('the apps glyph has one definition', () => {
  it('CategoryIcon delegates its apps case to AppsIcon', () => {
    expect((CategoryIcon({ category: 'apps' }) as VNode).type).toBe(AppsIcon);
  });

  it('CategoryIcon draws no apps shape of its own', () => {
    const arm = source('../CategoryIcon.tsx').split("case 'apps':")[1]?.split('case ')[0] ?? '';
    expect(arm, "expected a `case 'apps':` arm").not.toBe('');
    expect(arm).not.toMatch(/<(rect|path|circle|polyline|polygon|line)\b/);
    expect(arm).toContain('AppsIcon');
  });

  it('AppsIcon sizes itself, because one of its slots has no CSS to size it', () => {
    const svg = AppsIcon() as VNode<Record<string, unknown>>;
    expect(svg.props.width).toBe('1rem');
    expect(svg.props.height).toBe('1rem');
    const sized = AppsIcon({ size: '2rem' }) as VNode<Record<string, unknown>>;
    expect(sized.props.width).toBe('2rem');
  });
});

describe('the coding-agent App row', () => {
  const iconProps = (folder: string) =>
    (resolveAppInfo(folder)?.icon as VNode<Record<string, unknown>>);

  it('hands the listed icon to AppIcon', () => {
    appsList.value = {
      status: 'loaded',
      data: [{ id: 'habit-tracker', name: 'Habit Tracker', icon: 'assets/icon.svg' }],
    } as typeof appsList.value;
    const info = resolveAppInfo('data/apps/habit-tracker');
    expect(info?.name).toBe('Habit Tracker');
    const node = iconProps('data/apps/habit-tracker');
    expect(node.type).toBe(AppIcon);
    expect(node.props).toMatchObject({ appId: 'habit-tracker', name: 'Habit Tracker', icon: 'assets/icon.svg' });
  });

  it('shows the monogram tile for an app with no icon', () => {
    appsList.value = {
      status: 'loaded',
      data: [{ id: 'habit-tracker', name: 'Habit Tracker' }],
    } as typeof appsList.value;
    const node = iconProps('data/apps/habit-tracker');
    expect(node.type).toBe(AppIcon);
    expect(node.props.icon).toBeUndefined();
  });

  it('a failed appsList fetch says so in the name and still marks the row', () => {
    appsList.value = { status: 'failed', error: 'boom' };
    const info = resolveAppInfo('data/apps/habit-tracker');
    expect(info?.failed).toBe(true);
    expect(info?.name).toContain('apps failed to load');
    expect(iconProps('data/apps/habit-tracker').type).toBe(AppIcon);
  });

  it('a still-loading appsList marks the row without claiming failure', () => {
    appsList.value = { status: 'loading' };
    const info = resolveAppInfo('data/apps/habit-tracker');
    expect(info?.failed).toBeUndefined();
    expect(info?.name).toBe('habit-tracker');
    expect(iconProps('data/apps/habit-tracker').type).toBe(AppIcon);
  });
});
