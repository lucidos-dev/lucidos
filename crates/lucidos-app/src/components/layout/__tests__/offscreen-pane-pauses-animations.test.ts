import { describe, it, expect } from 'vitest';
import { isOffscreenPane } from '../MobileSwipeContainer';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';


const here: string = dirname(fileURLToPath(import.meta.url));
const swipeSrc = readFileSync(resolve(here, '../MobileSwipeContainer.tsx'), 'utf-8');
const mobileCss = (readFileSync(resolve(here, '../../../styles/mobile.css'), 'utf-8') as string)
  .replace(/\/\*[\s\S]*?\*\//g, '');

describe('isOffscreenPane', () => {
  it('is false for the current pane only', () => {
    expect(isOffscreenPane('thread', 'thread')).toBe(false);
    expect(isOffscreenPane('threads', 'thread')).toBe(true);
    expect(isOffscreenPane('content', 'thread')).toBe(true);
  });
});

describe('an off-screen pane', () => {
  it('is marked from the current pane signal', () => {
    expect(swipeSrc).toMatch(/data-offscreen=\{isOffscreenPane\(v, mobileView\.value\) \? '' : undefined\}/);
  });

  it('pauses every animation inside it, pseudo-elements included', () => {
    const rule = mobileCss.match(/([^{}]*\[data-offscreen\][^{}]*)\{([^}]*)\}/);
    if (!rule) throw new Error('no rule keyed on [data-offscreen] in mobile.css');
    const selectors = rule[1].split(',').map((s: string) => s.trim());
    for (const part of ['*', '*::before', '*::after']) {
      expect(selectors).toContain(`.mobile-swipe-pane[data-offscreen] ${part}`);
    }
    expect(rule[2]).toMatch(/animation-play-state\s*:\s*paused/);
  });

  it('outranks any selector whose animation shorthand resets the play state', () => {
    const rule = mobileCss.match(/\[data-offscreen\][^{}]*\{([^}]*)\}/);
    expect(rule?.[1]).toMatch(/animation-play-state\s*:\s*paused\s*!important/);
  });
});
