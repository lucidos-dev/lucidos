import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

// The content pane's navigation cover.
//
// Opening an app has faded in from an opaque theme surface since `.app-ui-cover`
// (see apps/AppUiInline.test.ts): the frame is hidden until it has something to
// show, so the pane goes theme-background to app with no swap frame in between.
// Every OTHER content-pane navigation hard-cut. A view switch unmounts the old
// subtree, mounts a lazy chunk that may not have arrived yet, restores a
// remembered scrollTop and lets the incoming view's own skeleton settle, and all
// of it landed in front of the user at once.
//
// So the cover is generic now: `.content-nav-cover` is mounted by ContentPane on
// every change of the view key and fades out on its own. The two covers differ
// only in what ends them, and that difference is the point. The app cover waits
// on the frame's `load`, because it is hiding a document the host does not
// author and cannot otherwise time. This one is hiding the switch frame alone,
// so it never waits on the arriving view: content shows through as soon as it
// paints, and a slow view uncovers its own skeleton rather than being withheld.
//
// The frontend test environment is deliberately non-jsdom, so there is no
// rendered DOM to assert against. Pin the wiring in source, the same approach as
// content-pane-ios-repaint.test.ts and AppUiInline.test.ts.

const here: string = dirname(fileURLToPath(import.meta.url));
const src = readFileSync(resolve(here, '../ContentPane.tsx'), 'utf-8');
const css = readFileSync(resolve(here, '../../../styles/panels/shell.css'), 'utf-8');

describe('content pane navigation cover', () => {
  it('mounts a cover on every content-pane navigation', () => {
    expect(src).toMatch(/content-nav-cover/);
  });

  it('drives the cover off the same view key as the scroll memory', () => {
    // One definition of "the pane navigated". A second notion of view identity
    // would let the two disagree: a scroll restore under an uncovered pane, or
    // a cover over a view that never changed.
    expect(src).toMatch(/const viewKey = contentViewKey\(active, overlay\)/);
    expect(src).toMatch(/coveredKeyRef\.current === viewKey/);
    expect(src).toMatch(/setCoverKey\(viewKey\)/);
  });

  it('keys the cover element on the view it covers', () => {
    // A navigation arriving mid-fade must restart from opaque rather than
    // inherit the outgoing cover's progress, and a keyed element is also what
    // makes the CSS animation replay at all: a class-toggled transition needs
    // the opaque state to reach the screen before the clearing class lands,
    // which from a Preact commit is a double-rAF race it can lose silently.
    expect(src).toMatch(/<div key=\{coverKey\} class="content-nav-cover"/);
    expect(css).toMatch(/\.content-nav-cover\s*\{[^}]*animation:\s*content-nav-cover-clear/);
  });

  it('unmounts the cover on a fuse, never on the animation alone', () => {
    // `prefers-reduced-motion: reduce` drops the animation, so an
    // `animationend`-driven unmount would never fire and the pane would stay
    // covered forever.
    expect(src).toMatch(
      /setTimeout\(\(\) => setCoverKey\(null\), scaledDurationMs\(NAV_COVER_ANIM_MS\) \+ NAV_COVER_SLACK_MS\)/,
    );
    expect(src).not.toMatch(/onAnimationEnd/);
  });

  it('scales the fuse with the animation-speed slider, and only the animation half', () => {
    // The cover clears on `animation: … var(--duration-normal)`, and that token
    // is scaled by the slider, so a fixed 250ms fuse would unmount the cover a
    // tenth of the way into its own fade at 0.1x. The slack is a fixed safety
    // margin rather than animation, so it stays outside the scaled term.
    expect(src).toMatch(/const NAV_COVER_ANIM_MS = 200;/);
    expect(src).toMatch(/const NAV_COVER_SLACK_MS = 50;/);
    // The base must be the 1x value of the token the CSS actually uses.
    expect(css).toMatch(
      /\.content-nav-cover\s*\{[^}]*animation:\s*content-nav-cover-clear var\(--duration-normal\)/,
    );
  });

  it('does not cover a pane that is navigating to nothing', () => {
    // No arriving view to hide, so a cover would be a flash of background
    // over a pane on its way to empty.
    expect(src).toMatch(/if \(viewKey === null\) \{ setCoverKey\(null\); return; \}/);
  });

  it('renders the cover outside the scrolling body', () => {
    // Inside `.content-pane-body` the cover would scroll away with the content
    // it is covering, and would not hide the scrollTop being restored under it.
    const body = src.match(/<div\s+class=\{`content-pane-body[\s\S]*?\n {6}<\/div>/)?.[0] ?? '';
    // Guard the guard: an unmatched slice would make the assertion below pass
    // for the wrong reason.
    expect(body).toMatch(/content-pane-body/);
    expect(body).not.toMatch(/content-nav-cover/);
    expect(src).toMatch(/content-nav-cover/);
  });

  it('paints the cover with the theme background, opaque and click-through', () => {
    const rule = css.match(/\.content-nav-cover\s*\{[^}]*\}/)?.[0] ?? '';
    expect(rule).toMatch(/position:\s*absolute/);
    expect(rule).toMatch(/inset:\s*0/);
    expect(rule).toMatch(/background:\s*var\(--bg-primary\)/);
    expect(rule).toMatch(/pointer-events:\s*none/);
    // The cover exists to hide the swap frame, so it starts fully opaque.
    expect(css).toMatch(/@keyframes content-nav-cover-clear\s*\{[^}]*from\s*\{\s*opacity:\s*1/);
  });

  it('stays clear of a fullscreen app', () => {
    // `.app-ui-fullscreen` escapes the pane at --z-app-fullscreen; a cover that
    // outranked it would black out a fullscreen app on the next navigation.
    const rule = css.match(/\.content-nav-cover\s*\{[^}]*\}/)?.[0] ?? '';
    const z = rule.match(/z-index:\s*(\d+)/)?.[1];
    expect(z).toBeDefined();
    expect(Number(z)).toBeLessThan(2250);
  });

  it('is transparent, not merely unanimated, under reduced motion', () => {
    // The element is still mounted for the fuse duration, so dropping the
    // animation without dropping the opacity would park an opaque panel over
    // the view for exactly as long as the animation would have run.
    const rule = css.match(
      /@media \(prefers-reduced-motion: reduce\)\s*\{\s*\.content-nav-cover\s*\{[^}]*\}/,
    )?.[0] ?? '';
    expect(rule).toMatch(/animation:\s*none/);
    expect(rule).toMatch(/opacity:\s*0/);
  });
});
