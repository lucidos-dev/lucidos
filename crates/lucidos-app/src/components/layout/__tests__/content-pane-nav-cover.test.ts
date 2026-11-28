import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
import { REDUCED_MOTION_ROOT, cssRules, selectorList } from '../../../styles/__tests__/css-rule-helpers';


const here: string = dirname(fileURLToPath(import.meta.url));
const src = readFileSync(resolve(here, '../ContentPane.tsx'), 'utf-8');
const cover = readFileSync(resolve(here, '../../shared/NavigationCover.tsx'), 'utf-8');
const css = readFileSync(resolve(here, '../../../styles/global/host-components.css'), 'utf-8');
const shellRules = cssRules(readFileSync(resolve(here, '../../../styles/panels/shell.css'), 'utf-8'));
const hostRules = cssRules(css);

describe('content pane navigation cover', () => {
  it('mounts the shared cover on every content-pane navigation', () => {
    expect(src).toMatch(/import \{ NavigationCover \} from '\.\.\/shared\/NavigationCover'/);
    expect(src).toMatch(/<NavigationCover viewKey=\{viewKey\} \/>/);
  });

  it('drives the cover off the same view key as the scroll memory', () => {
    expect(src).toMatch(/const viewKey = contentViewKey\(active, overlay, subview\)/);
    expect(src).toMatch(/useScrollMemory\(bodyRef, viewKey \?/);
  });

  it('reports the navigation mark on a change only, never on the first render', () => {
    expect(src).toMatch(/if \(reportedKeyRef\.current === viewKey\) return;/);
    expect(src).toMatch(/reportNavigation\('content-view', viewKey\)/);
  });

  it('keys the cover element on the view it covers', () => {
    expect(cover).toMatch(/<div key=\{arriving\} class=\{NAV_COVER_MOTIONS\[motion\]\.class\}/);
    expect(cover).toMatch(/arrive: \{ class: 'nav-cover', animMs: 200 \}/);
    expect(css).toMatch(/\.nav-cover\s*\{[^}]*animation:\s*nav-cover-clear/);
  });

  it('ends an arrival on a fuse, never on the animation alone', () => {
    expect(cover).toMatch(/setTimeout\(\(\) => setArriving\(null\), navCoverFuseMs\(motion\)\)/);
    expect(cover).toMatch(/return scaledDurationMs\(NAV_COVER_MOTIONS\[motion\]\.animMs\) \+ NAV_COVER_SLACK_MS;/);
    expect(cover).not.toMatch(/onAnimationEnd/);
  });

  it('scales the fuse with the animation-speed slider, and only the animation half', () => {
    expect(cover).toMatch(/arrive: \{ class: 'nav-cover', animMs: 200 \}/);
    expect(cover).toMatch(/const NAV_COVER_SLACK_MS = 50;/);
    expect(css).toMatch(/\.nav-cover\s*\{[^}]*animation:\s*nav-cover-clear var\(--duration-normal\)/);
  });

  it('does not cover a pane that is navigating to nothing', () => {
    expect(cover).toMatch(/if \(viewKey === null\) \{ setArriving\(null\); return; \}/);
  });

  it('renders the cover outside the scrolling body', () => {
    const body = src.match(/<div\s+class=\{`content-pane-body[\s\S]*?\n {6}<\/div>/)?.[0] ?? '';
    expect(body).toMatch(/content-pane-body/);
    expect(body).not.toMatch(/NavigationCover/);
  });

  it('paints the cover with the theme background, opaque and click-through', () => {
    const rule = css.match(/\.nav-cover\s*\{[^}]*\}/)?.[0] ?? '';
    expect(rule).toMatch(/position:\s*absolute/);
    expect(rule).toMatch(/inset:\s*0/);
    expect(rule).toMatch(/background:\s*var\(--bg-primary\)/);
    expect(rule).toMatch(/pointer-events:\s*none/);
    expect(css).toMatch(/@keyframes nav-cover-clear\s*\{[^}]*from\s*\{\s*opacity:\s*1/);
  });

  it('stays clear of a fullscreen app', () => {
    const host = shellRules.find(r => r.selector === '.content-pane > .nav-cover');
    const z = host?.props.get('z-index');
    expect(z).toBeDefined();
    expect(Number(z)).toBeLessThan(2250);
  });

  it('is transparent, not merely unanimated, under reduced motion', () => {
    const reduce = hostRules.find(r => selectorList(r.selector).includes(`${REDUCED_MOTION_ROOT} .nav-cover`));
    expect(reduce?.props.get('animation')).toBe('none');
    expect(reduce?.props.get('opacity')).toBe('0');
  });
});

describe('the pane titles switch word at once, with no fade of their own', () => {
  const desktop = readFileSync(resolve(here, '../AppHeader.tsx'), 'utf-8');
  const mobile = readFileSync(resolve(here, '../MobileAppHeader.tsx'), 'utf-8');
  const headerControls = readFileSync(resolve(here, '../ThreadsHeaderControls.tsx'), 'utf-8');

  it('no rule animates a title in', () => {
    expect(css).not.toMatch(/nav-arrive|crossfade-dip/);
  });

  it.each([['desktop', desktop], ['mobile', mobile]])('%s: the content title is a plain span', (_what, header) => {
    expect(header).not.toMatch(/useContentTitleArrival|titleFade|key=\{titleKey\}/);
  });

  it('the drawer title is one plain word, not a stack', () => {
    const title = headerControls.slice(headerControls.indexOf('export function ThreadsPaneTitle'));
    expect(title).toMatch(/const text = threadFilterPanelOpen\.value \? 'Filters' : GROUPING_LABEL\[drawerGrouping\.value\];/);
    expect(title).toMatch(/return <span ref=\{ref\} class=\{className\} data-cramped=\{[^}]+\}>\{text\}<\/span>/);
    expect(title).not.toMatch(/CrossfadeStack/);
  });
});
