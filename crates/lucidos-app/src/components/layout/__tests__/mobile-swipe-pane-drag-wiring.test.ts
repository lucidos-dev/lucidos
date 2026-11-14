import { describe, it, expect } from 'vitest';
import { RUBBER_BAND, rubberBanded } from '../MobileSwipeContainer';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

// The strip's rules live in `PaneDrag` and are tested in `paneDrag.test.ts`,
// stranded touches included. The frontend test environment has no DOM, so
// this file pins the strip's wiring to it in source.
// See `docs/plans/2026-10-11-a-pane-swipe-reaches-no-invalid-state.md`.

const here: string = dirname(fileURLToPath(import.meta.url));
const swipeSrc = readFileSync(resolve(here, '../MobileSwipeContainer.tsx'), 'utf-8');
const stripTrackSrc = swipeSrc.match(/export function stripTrack[\s\S]*?\n\}\n/)?.[0] ?? '';
const componentSrc = swipeSrc.slice(swipeSrc.indexOf('export function MobileSwipeContainer'));
const headerSrc = readFileSync(resolve(here, '../AppHeader.tsx'), 'utf-8');
const viewerSrc = readFileSync(resolve(here, '../../shared/ImagePopup.tsx'), 'utf-8');

describe('the strip drives its drag through PaneDrag', () => {
  it('takes host touches through attachPaneDragTouches, with the strip gate', () => {
    expect(componentSrc).toMatch(/attachPaneDragTouches\(el, drag, \{ gate: gateStripTouch \}\)/);
    expect(swipeSrc).not.toMatch(/el\.addEventListener\('touch/);
  });

  it('routes every frame message, the frame going away included', () => {
    const frames = componentSrc.match(/subscribeFrameSwipes\([\s\S]*?\}\), \[\]\);/)?.[0] ?? '';
    expect(frames).toMatch(/drag\.frameDrag\(source, message\.dx\)/);
    expect(frames).toMatch(/drag\.frameRelease\(source, message\.paneDelta\)/);
    expect(frames).toMatch(/drag\.frameGone\(source\)/);
  });

  it('lets only a frame inside the strip lead it', () => {
    expect(componentSrc).toMatch(/isFrameInside\(containerRef\.current, source\)/);
  });

  it('ends a drag on a navigation, a resize and a fullscreen change', () => {
    expect(componentSrc).toMatch(/drag\.interrupt\(mountedRef\.current\);[\s\S]*?\}, \[mobileView\.value\]\);/);
    expect(componentSrc).toMatch(/lastPaneWidth = paneWidth;\s*drag\.interrupt\(false\);/);
    expect(componentSrc).toMatch(/drag\.interrupt\(false\);\s*wasPseudoFullscreen\.current/);
  });

  it('decides the edge guard from the touch that started', () => {
    const gate = swipeSrc.match(/function gateStripTouch[\s\S]*?\n\}\n/)?.[0] ?? '';
    expect(gate).toMatch(/clientX: touch\.clientX/);
    expect(gate).not.toMatch(/touches\[0\]/);
  });
});

describe('only the strip track writes the transform and transition', () => {
  it('has no other writer in the component but the transitionend net', () => {
    expect(componentSrc).not.toMatch(/style\.transition\s*=/);
    expect(componentSrc.match(/\.style\.transform = /g)).toEqual(['.style.transform = ']);
    expect(componentSrc).toMatch(/track\.style\.transform = correctValue;/);
  });

  // The transitionend net may correct a resting strip, never a held one.
  it('leaves a held strip to the drag in the transitionend net', () => {
    expect(componentSrc).toMatch(/propertyName !== 'transform' \|\| drag\.holding\) return;/);
  });

  it('re-enables a snapped transition only when nothing holds the strip', () => {
    const deferred = stripTrackSrc.match(/requestAnimationFrame\([\s\S]*?\}\);/g) ?? [];
    expect(deferred.length).toBe(1);
    expect(deferred[0]).toMatch(/if \(!isHeld\(\) && trackRef\.current\)/);
    // The snap must reach a style pass before that frame turns it back on.
    const rest = stripTrackSrc.match(/rest\(animate\) \{[\s\S]*?\n {4}\},/)?.[0] ?? '';
    expect(rest).toMatch(/style\.transition = 'none';[\s\S]*?void track\.offsetWidth;[\s\S]*?requestAnimationFrame/);
    expect(swipeSrc.match(/requestAnimationFrame\(/g)?.length).toBe(1);
  });

  it('turns the transition off for every held frame', () => {
    const follow = stripTrackSrc.match(/follow\(dx\) \{[\s\S]*?\n {4}\},/)?.[0] ?? '';
    expect(follow).toMatch(/style\.transition = 'none'/);
  });

  // ADR 0420: the strip is overflow: hidden, so a focus() can scroll it.
  it('resets the scroll offset whenever it rests', () => {
    const rest = stripTrackSrc.match(/rest\(animate\) \{[\s\S]*?\n {4}\},/)?.[0] ?? '';
    expect(rest).toMatch(/container\.scrollLeft = 0;/);
    expect(swipeSrc).toMatch(/if \(container\.scrollLeft !== 0\) container\.scrollLeft = 0;/);
  });

  it('catches a drag where the strip is on screen', () => {
    const grab = stripTrackSrc.match(/grab\(\) \{[\s\S]*?\n {4}\},/)?.[0] ?? '';
    expect(grab).toMatch(/base = computedTranslateX\(track\)/);
  });
});

// The header keeps no visual feedback and gains the strip's touch rules.
describe('the header swipe uses the same machine', () => {
  it('drives a PaneDrag with a track that draws nothing', () => {
    const hook = headerSrc.match(/function useHeaderPaneSwipe[\s\S]*?\n\}\n/)?.[0] ?? '';
    expect(hook).toMatch(/new PaneDrag\(HEADER_TRACK,/);
    expect(hook).toMatch(/attachPaneDragTouches\(el, drag,/);
    expect(headerSrc).not.toMatch(/SwipeTouch/);
  });
});

// Defect 11: the image viewer committed a swipe on a cancel.
describe('the image viewer never commits on a cancel', () => {
  it('springs a cancelled swipe back', () => {
    expect(viewerSrc).toMatch(/addEventListener\('touchcancel', handleTouchCancel,/);
    const cancel = viewerSrc.match(/function handleTouchCancel[\s\S]*?\n {4}\}/)?.[0] ?? '';
    expect(cancel).toMatch(/swipe\.cancel\(\);\s*settleTouch\(e, 0, wasHorizontal\);/);
  });

  it('reads the strip offset with the shared parser', () => {
    expect(viewerSrc).toMatch(/computedTranslateX\(strip\)/);
    expect(viewerSrc).not.toMatch(/function parseTranslateX/);
  });
});

describe('rubberBanded', () => {
  const W = 300;

  it('follows the finger inside the strip', () => {
    expect(rubberBanded(-150, W)).toBe(-150);
  });

  it('scales the drag past either end by the rubber band', () => {
    expect(rubberBanded(30, W)).toBeCloseTo(30 * RUBBER_BAND);
    expect(rubberBanded(-2 * W - 30, W)).toBeCloseTo(-2 * W - 30 * RUBBER_BAND);
  });
});
