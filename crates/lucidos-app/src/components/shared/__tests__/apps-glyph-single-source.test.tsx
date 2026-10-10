/**
 * There is exactly ONE apps glyph.
 *
 * Three surfaces mark an app: Search Everywhere's result rows and the
 * content-pane back/forward history menu (both through `CategoryIcon`'s `apps`
 * case), and the message route panel's fallback for an app whose manifest
 * declares no icon of its own. Before 2026-08-13 the first two drew a 2x2
 * rounded tile grid inline while the third rendered a 📦, so the same concept
 * wore two marks and neither knew about the other.
 *
 * They are one `AppsIcon` now, and this pins it from both ends: `CategoryIcon`
 * must delegate rather than re-inline, and `resolveAppInfo`'s fallback must be
 * that same component. Without the first half the grid can quietly come back
 * next to a package; without the second the route panel drifts on its own.
 *
 * The `CategoryIcon` half is a source scan because the point is that the file
 * contains no second drawing, which a rendered comparison cannot express: two
 * hand-written copies of one path render identically and pass.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
import type { VNode } from 'preact';
import { CategoryIcon } from '../CategoryIcon';
import { AppsIcon } from '../icons';
import { resolveAppInfo } from '../../chat/MessageRoutePanel';
import { appsList } from '../../../store/store';

const CATEGORY_ICON_SRC = readFileSync(new URL('../CategoryIcon.tsx', import.meta.url), 'utf8');

describe('the apps glyph has one definition', () => {
  it('CategoryIcon delegates its apps case to AppsIcon', () => {
    expect((CategoryIcon({ category: 'apps' }) as VNode).type).toBe(AppsIcon);
  });

  it('CategoryIcon draws no apps shape of its own', () => {
    // The arm runs from `case 'apps':` to the next `case`, and must contain no
    // geometry: a `<rect>`/`<path>`/`<circle>` here is a second copy of the mark.
    const arm = CATEGORY_ICON_SRC.split("case 'apps':")[1]?.split('case ')[0] ?? '';
    expect(arm, "expected a `case 'apps':` arm").not.toBe('');
    expect(arm).not.toMatch(/<(rect|path|circle|polyline|polygon|line)\b/);
    expect(arm).toContain('AppsIcon');
  });

  it('the route panel fallback is the same component', () => {
    appsList.value = { status: 'loaded', data: [] };
    const info = resolveAppInfo('data/apps/habit-tracker');
    expect(info?.name).toBe('habit-tracker');
    expect((info?.icon as VNode).type).toBe(AppsIcon);
  });

  it('AppsIcon sizes itself, because one of its slots has no CSS to size it', () => {
    // `.search-everywhere-result-icon` carries no rule anywhere in styles/.
    // That surface has always sized its glyphs from the `width`/`height`
    // attributes `CategoryIcon`'s props spread puts on each <svg>. Delegating
    // the apps case to a component that omitted them painted an app hit at the
    // default replaced-element box (~300px) instead of 1rem. The other two
    // consumers do size their slot in CSS and so never showed it.
    const svg = AppsIcon() as VNode<Record<string, unknown>>;
    expect(svg.props.width).toBe('1rem');
    expect(svg.props.height).toBe('1rem');
    // A caller with its own CSS still wins: a rule beats a presentation
    // attribute, which is what keeps the route panel at --icon-size-md.
    const sized = AppsIcon({ size: '2rem' }) as VNode<Record<string, unknown>>;
    expect(sized.props.width).toBe('2rem');
  });
});

describe("an app's own manifest icon is user content and passes through", () => {
  it('a declared icon is rendered verbatim, never replaced by the fallback', () => {
    appsList.value = {
      status: 'loaded',
      data: [{ id: 'habit-tracker', name: 'Habit Tracker', icon: '\u{1F331}' }],
    } as typeof appsList.value;
    const info = resolveAppInfo('data/apps/habit-tracker');
    expect(info?.icon).toBe('\u{1F331}');
    expect(info?.name).toBe('Habit Tracker');
  });

  it('a loaded app with an empty icon falls back', () => {
    appsList.value = {
      status: 'loaded',
      data: [{ id: 'habit-tracker', name: 'Habit Tracker', icon: '' }],
    } as typeof appsList.value;
    expect((resolveAppInfo('data/apps/habit-tracker')?.icon as VNode).type).toBe(AppsIcon);
  });

  it('a failed appsList fetch says so in the name and still marks the row', () => {
    appsList.value = { status: 'failed', error: 'boom' };
    const info = resolveAppInfo('data/apps/habit-tracker');
    expect(info?.failed).toBe(true);
    expect(info?.name).toContain('apps failed to load');
    expect((info?.icon as VNode).type).toBe(AppsIcon);
  });

  it('a still-loading appsList marks the row without claiming failure', () => {
    appsList.value = { status: 'loading' };
    const info = resolveAppInfo('data/apps/habit-tracker');
    expect(info?.failed).toBeUndefined();
    expect(info?.name).toBe('habit-tracker');
    expect((info?.icon as VNode).type).toBe(AppsIcon);
  });
});
