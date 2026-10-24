import { readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';

/** The suite runs on calm motion (`reducedMotion: 'reduce'` in
 *  playwright.config.ts), so a spec that measures motion would pass with
 *  nothing moving. Every spec that reads motion therefore names the motion it
 *  wants in a `test.use`: `'no-preference'` to see real motion, or `'reduce'`
 *  when it only waits motion out. */

const E2E_DIR = __dirname;

/** What reading motion looks like in a spec. */
const MOTION_READS: { name: string; matches: (src: string) => boolean }[] = [
  { name: 'getAnimations()', matches: src => src.includes('getAnimations(') },
  { name: 'animationName', matches: src => src.includes('animationName') },
  { name: 'the animation speed slider', matches: src => src.includes('animation-speed') },
  {
    name: 'a frame sampler',
    matches: src => src.includes('requestAnimationFrame') && src.includes('performance.now()'),
  },
];

const STANCE = /test\.use\(\{[^)]*\breducedMotion:/;

function motionReads(src: string): string[] {
  return MOTION_READS.filter(read => read.matches(src)).map(read => read.name);
}

function namesMotionStance(src: string): boolean {
  return STANCE.test(src);
}

describe('motion stance', () => {
  it('every spec that reads motion names the motion it wants', () => {
    const missing = readdirSync(E2E_DIR)
      .filter(file => file.endsWith('.spec.ts'))
      .flatMap(file => {
        const src = readFileSync(join(E2E_DIR, file), 'utf8');
        const reads = motionReads(src);
        return reads.length > 0 && !namesMotionStance(src) ? [`${file}: ${reads.join(', ')}`] : [];
      });
    expect(missing, "add test.use({ reducedMotion: 'no-preference' }) or 'reduce'").toEqual([]);
  });

  it('spots each kind of motion read', () => {
    expect(motionReads('el.getAnimations().length')).toEqual(['getAnimations()']);
    expect(motionReads('getComputedStyle(el).animationName')).toEqual(['animationName']);
    expect(motionReads("localStorage.setItem('lucidos-animation-speed-slider', '-10')"))
      .toEqual(['the animation speed slider']);
    expect(motionReads('const t = performance.now(); requestAnimationFrame(tick);')).toEqual(['a frame sampler']);
    expect(motionReads('await new Promise(r => requestAnimationFrame(r));')).toEqual([]);
  });

  it('accepts a stance in any test.use, alone or beside other options', () => {
    expect(namesMotionStance("test.use({ reducedMotion: 'no-preference' });")).toBe(true);
    expect(namesMotionStance("test.use({ viewport: { width: 1, height: 1 }, reducedMotion: 'reduce' });")).toBe(true);
    expect(namesMotionStance("await page.emulateMedia({ reducedMotion: 'reduce' });")).toBe(false);
  });
});
