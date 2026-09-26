import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
import { REDUCED_MOTION_ROOT, cssRules, selectorList } from '../../../styles/__tests__/css-rule-helpers';

// The content pane's navigation cover.
//
// Opening an app has faded in from an opaque theme surface since `.app-ui-cover`
// (see apps/AppUiInline.test.ts). Every OTHER content-pane navigation used to
// hard-cut: a view switch unmounts the old subtree, mounts a lazy chunk, restores
// a remembered scrollTop and lets the incoming skeleton settle, all at once.
//
// So ContentPane mounts the shared `NavigationCover` on every change of the view
// key. The two covers differ only in what ends them. The app cover waits on the
// frame's `load`, because it hides a document the host does not author. This one
// hides the switch frame alone, so it never waits on the arriving view.
//
// The frontend test environment is deliberately non-jsdom here, so this pins the
// wiring in source. The component's own behaviour is pinned in jsdom by
// components/drawer/__tests__/filter-panel-fade.test.tsx.

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
    // One definition of "the pane navigated". A second notion of view identity
    // would let the two disagree: a scroll restore under an uncovered pane, or
    // a cover over a view that never changed.
    expect(src).toMatch(/const viewKey = contentViewKey\(active, overlay, subview\)/);
    expect(src).toMatch(/useScrollMemory\(bodyRef, viewKey \?/);
  });

  it('reports the navigation mark on a change only, never on the first render', () => {
    expect(src).toMatch(/if \(reportedKeyRef\.current === viewKey\) return;/);
    expect(src).toMatch(/reportNavigation\('content-view', viewKey\)/);
  });

  it('keys the cover element on the view it covers', () => {
    // A navigation arriving mid-fade must restart from opaque rather than
    // inherit the outgoing cover's progress, and a keyed element is also what
    // makes the CSS animation replay at all: a class-toggled transition needs
    // the opaque state to reach the screen before the clearing class lands,
    // which from a Preact commit is a double-rAF race it can lose silently.
    expect(cover).toMatch(/<div key=\{arriving\} class="nav-cover"/);
    expect(css).toMatch(/\.nav-cover\s*\{[^}]*animation:\s*nav-cover-clear/);
  });

  it('ends an arrival on a fuse, never on the animation alone', () => {
    // Reduced motion drops the animation, so an `animationend`-driven unmount
    // would never fire and the pane would stay covered forever.
    expect(cover).toMatch(
      /setTimeout\(\(\) => setArriving\(null\), scaledDurationMs\(NAV_COVER_ANIM_MS\) \+ NAV_COVER_SLACK_MS\)/,
    );
    expect(cover).not.toMatch(/onAnimationEnd/);
  });

  it('scales the fuse with the animation-speed slider, and only the animation half', () => {
    // The cover clears on `animation: … var(--duration-normal)`, and that token
    // is scaled by the slider, so a fixed 250ms fuse would unmount the cover a
    // tenth of the way into its own fade at 0.1x. The slack is a fixed safety
    // margin rather than animation, so it stays outside the scaled term.
    expect(cover).toMatch(/const NAV_COVER_ANIM_MS = 200;/);
    expect(cover).toMatch(/const NAV_COVER_SLACK_MS = 50;/);
    // The base must be the 1x value of the token the CSS actually uses.
    expect(css).toMatch(/\.nav-cover\s*\{[^}]*animation:\s*nav-cover-clear var\(--duration-normal\)/);
    expect(css).toMatch(/\.nav-arrive\s*\{[^}]*animation:\s*nav-arrive var\(--duration-normal\)/);
  });

  it('does not cover a pane that is navigating to nothing', () => {
    // No arriving view to hide, so a cover would be a flash of background
    // over a pane on its way to empty.
    expect(cover).toMatch(/if \(viewKey === null\) \{ setArriving\(null\); return; \}/);
  });

  it('renders the cover outside the scrolling body', () => {
    // Inside `.content-pane-body` the cover would scroll away with the content
    // it is covering, and would not hide the scrollTop being restored under it.
    const body = src.match(/<div\s+class=\{`content-pane-body[\s\S]*?\n {6}<\/div>/)?.[0] ?? '';
    // Guard the guard: an unmatched slice would make the assertion below pass
    // for the wrong reason.
    expect(body).toMatch(/content-pane-body/);
    expect(body).not.toMatch(/NavigationCover/);
  });

  it('paints the cover with the theme background, opaque and click-through', () => {
    const rule = css.match(/\.nav-cover\s*\{[^}]*\}/)?.[0] ?? '';
    expect(rule).toMatch(/position:\s*absolute/);
    expect(rule).toMatch(/inset:\s*0/);
    expect(rule).toMatch(/background:\s*var\(--bg-primary\)/);
    expect(rule).toMatch(/pointer-events:\s*none/);
    // The cover exists to hide the swap frame, so it starts fully opaque.
    expect(css).toMatch(/@keyframes nav-cover-clear\s*\{[^}]*from\s*\{\s*opacity:\s*1/);
  });

  it('stays clear of a fullscreen app', () => {
    // `.app-ui-fullscreen` escapes the pane at --z-app-fullscreen; a cover that
    // outranked it would black out a fullscreen app on the next navigation.
    const host = shellRules.find(r => r.selector === '.content-pane > .nav-cover');
    const z = host?.props.get('z-index');
    expect(z).toBeDefined();
    expect(Number(z)).toBeLessThan(2250);
  });

  it('is transparent, not merely unanimated, under reduced motion', () => {
    // The element is still mounted for the fuse duration, so dropping the
    // animation without dropping the opacity would park an opaque panel over
    // the view for exactly as long as the animation would have run.
    const reduce = hostRules.find(r => selectorList(r.selector).includes(`${REDUCED_MOTION_ROOT} .nav-cover`));
    expect(reduce?.props.get('animation')).toBe('none');
    expect(reduce?.props.get('opacity')).toBe('0');
  });
});

describe('the content pane title arrives with its view', () => {
  const helpers = readFileSync(resolve(here, '../headerHelpers.ts'), 'utf-8');
  const desktop = readFileSync(resolve(here, '../AppHeader.tsx'), 'utf-8');
  const mobile = readFileSync(resolve(here, '../MobileAppHeader.tsx'), 'utf-8');

  it('keys the title on the pane\'s own view key', () => {
    expect(helpers).toMatch(/const titleKey = contentViewKey\(activeMenuItem\.value, panelOverlay\.value, settingsSubview\.value\)/);
    expect(helpers).toMatch(/useArrivalFade\(titleKey\)/);
  });

  it.each([['desktop', desktop], ['mobile', mobile]])('%s: the title element is keyed and fades on arrival', (_what, header) => {
    expect(header).toMatch(/const \{ titleKey, titleFade \} = useContentTitleArrival\(\);/);
    expect(header).toMatch(/key=\{titleKey\}/);
    expect(header).toMatch(/\$\{titleFade\}`\}/);
  });

  it('rests transparent while it fades, and shows at once under reduced motion', () => {
    const arrive = hostRules.find(r => r.selector === '.nav-arrive');
    expect(arrive?.props.get('opacity')).toBe('0');
    expect(css).toMatch(/@keyframes nav-arrive\s*\{\s*from\s*\{[^}]*\}\s*to\s*\{\s*opacity:\s*1/);
    const reduce = hostRules.find(r => selectorList(r.selector).includes(`${REDUCED_MOTION_ROOT} .nav-arrive`));
    expect(reduce?.props.get('animation')).toBe('none');
    expect(reduce?.props.get('opacity')).toBe('1');
  });
});
