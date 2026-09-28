import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

// Pull to refresh on the content pane. The gesture, the runner and the header
// spinner each have tests of their own. This pins the wiring between them,
// since the frontend test environment is deliberately non-jsdom.

const here: string = dirname(fileURLToPath(import.meta.url));
const pane = readFileSync(resolve(here, '../ContentPane.tsx'), 'utf-8');
const hostCss = readFileSync(resolve(here, '../../../styles/global/host-components.css'), 'utf-8');
const shellCss = readFileSync(resolve(here, '../../../styles/panels/shell.css'), 'utf-8');
const mobileHeader = readFileSync(resolve(here, '../MobileAppHeader.tsx'), 'utf-8');
const desktopHeader = readFileSync(resolve(here, '../AppHeader.tsx'), 'utf-8');
const contentActions = readFileSync(resolve(here, '../ContentHeaderActions.tsx'), 'utf-8');
const indicator = readFileSync(resolve(here, '../RefreshIndicator.tsx'), 'utf-8');
const mobileCss = readFileSync(resolve(here, '../../../styles/mobile.css'), 'utf-8');

describe('content pane pull to refresh', () => {
  it('tracks pulls on the pane body and runs the open panel\'s refresh', () => {
    expect(pane).toMatch(/trackPullToRefresh\(body, \{/);
    expect(pane).toMatch(/onPull: showPullTravel/);
    expect(pane).toMatch(/onRefresh: \(\) => void runPanelRefresh\(\)/);
  });

  it('draws the pull affordance outside the scrolling body', () => {
    // Inside the body it would scroll away with the content it sits over.
    const bodyClose = pane.lastIndexOf('</div>\n      {/* Outside `.content-pane-body`');
    expect(bodyClose).toBeGreaterThan(-1);
    expect(pane.indexOf('<PullRefreshAffordance />')).toBeGreaterThan(bodyClose);
  });

  it('leads the desktop row with Refresh, beside the hamburger', () => {
    // Out of the trailing cluster, so it never folds into the ⋯ menu.
    expect(desktopHeader).toMatch(/<HamburgerButton \/>\s*<ContentRefreshButton \/>/);
    expect(contentActions).not.toMatch(/busy/);
  });

  it('spins desktop Refresh until the check, and stops it under reduced motion', () => {
    const spinning = '\\.app-header \\.content-refresh-btn:not\\(\\[data-state="idle"\\]\\) svg:not\\(\\.refresh-check\\)';
    expect(shellCss).toMatch(new RegExp(`${spinning}\\s*\\{\\s*animation: spin`));
    expect(shellCss).toMatch(new RegExp(`:root\\[data-motion="reduce"\\] ${spinning}\\s*\\{\\s*animation: none;`));
    expect(shellCss).toMatch(/\.app-header \.content-refresh-btn\[data-state="done"\] \.refresh-check\s*\{[^}]*opacity: 1;/);
    // The old in-place spin on the trailing action is gone with it.
    expect(hostCss).not.toMatch(/is-busy/);
  });

  it('makes desktop Refresh inert while it spins or checks, keeping its tooltip', () => {
    // aria-disabled, not `disabled`: the real attribute takes the pointer, and the tooltip with it.
    expect(indicator).toMatch(/const busy = state !== 'idle';/);
    expect(indicator).toMatch(/onClick: busy \? undefined : \(\) => void runPanelRefresh\(\)/);
    expect(indicator).toMatch(/'aria-disabled': String\(busy\)/);
    expect(indicator).toMatch(/tooltip: 'Refresh',/);
    expect(shellCss).toMatch(/\.app-header \.icon-btn:hover:where\(:not\(:disabled, \[aria-disabled="true"\]\)\)/);
    expect(shellCss).toMatch(/\.app-header \.icon-btn\[aria-disabled="true"\]\s*\{\s*cursor: default;/);
  });

  it('spins a phone\'s running refresh in the leading slot, beside the hamburger', () => {
    // The trailing slot is the ⋯ trigger's, and a spinner there folds away.
    expect(mobileHeader).toMatch(/<HamburgerButton \/>\s*<MobileRefreshIndicator \/>/);
    // Spins only while shown, since a hidden spin still costs frames.
    const spinning = '\\.mobile-refresh-indicator:not\\(\\[data-state="idle"\\]\\) svg:not\\(\\.refresh-check\\)';
    expect(mobileCss).toMatch(new RegExp(`${spinning}\\s*\\{\\s*animation: spin`));
    expect(mobileCss).not.toMatch(/\.mobile-refresh-indicator svg\s*\{[^}]*animation/);
    expect(mobileCss).toMatch(new RegExp(`:root\\[data-motion="reduce"\\] ${spinning}\\s*\\{\\s*animation: none;`));
  });

  it('crossfades the spinner into a check once the refresh lands', () => {
    // Hidden outside `refreshing`, so it never fades back in over the leaving check.
    expect(mobileCss).toMatch(/\.mobile-refresh-indicator:not\(\[data-state="refreshing"\]\) svg:not\(\.refresh-check\)\s*\{\s*opacity: 0;/);
    expect(mobileCss).toMatch(/\.mobile-refresh-indicator\[data-state="done"\] \.refresh-check\s*\{\s*opacity: 1;/);
  });

  it('fades the check out at full size, on both rows', () => {
    // The exit resets `scale` only once the check is invisible, so it fades rather than shrinks.
    const exit = 'transition: opacity var\\(--duration-slow\\) ease-in-out, scale 0s var\\(--duration-slow\\);';
    expect(mobileCss).toMatch(new RegExp(`\\.mobile-refresh-indicator \\.refresh-check\\s*\\{[^}]*${exit}`));
    expect(shellCss).toMatch(new RegExp(`\\.app-header \\.content-refresh-btn \\.refresh-check\\s*\\{[^}]*${exit}`));
  });

  it('brings the desktop arrow back only once the check has faded', () => {
    expect(shellCss).toMatch(/\.app-header \.content-refresh-btn\[data-state="idle"\] svg:not\(\.refresh-check\)\s*\{\s*transition-delay: var\(--duration-slow\);/);
  });

  it('never fades the phone slot itself, which would compound the check\'s fade', () => {
    expect(mobileCss).not.toMatch(/\.mobile-refresh-indicator\s*\{[^}]*opacity/);
  });

  it('shrinks the spinner through `scale`, which the spin\'s `transform` leaves free', () => {
    expect(mobileCss).toMatch(/\.mobile-refresh-indicator svg\s*\{[^}]*transition: opacity var\(--duration-slow\) ease, scale var\(--duration-slow\) ease;/);
    expect(mobileCss).toMatch(/\.mobile-refresh-indicator:not\(\[data-state="refreshing"\]\) svg:not\(\.refresh-check\)\s*\{[^}]*scale: 0\.6;/);
  });

  it('blends the arrow toward the accent with the pull, never snapping at the threshold', () => {
    expect(shellCss).toMatch(/--pull-accent-mix: calc\(var\(--pull-progress, 0\) \* 100%\)/);
    expect(shellCss).toMatch(/color: color-mix\(in srgb, var\(--accent\) var\(--pull-accent-mix\)/);
    expect(shellCss).not.toMatch(/\.pull-refresh-affordance\.is-armed/);
  });

  it('turns the arrow with the whole pull, not the capped drop', () => {
    expect(shellCss).toMatch(/transform: rotate\(calc\(var\(--pull-turn, 0\) \* 270deg\)\)/);
  });

  it('lets the affordance spring back on a scaled duration token', () => {
    // A tokenized transition collapses with reduced motion by itself.
    expect(shellCss).toMatch(/\.pull-refresh-affordance \{[^}]*transition: transform var\(--duration-normal\)/);
  });
});
