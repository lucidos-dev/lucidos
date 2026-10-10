// @vitest-environment jsdom
/**
 * The client's one answer to "is motion reduced?", and where it lands.
 *
 * Every script caller reads `reducedMotion`, and every stylesheet keys on the
 * `data-motion` attribute published from it. So these cases pin the whole
 * contract: 3 preference values x the OS switch, the attribute, and the
 * duration scale that follows it.
 */
import { afterEach, describe, expect, it } from 'vitest';
import { effect } from '@preact/signals';
import { REDUCED_MOTION_DURATION_SCALE } from '@lucidos/appearance';
import {
  installMotionAttribute, isReducedMotion, motionPreference, osReducesMotion, reducedMotion,
} from './motion';
import { animationSpeed, durationScale } from '../store/store';

afterEach(() => {
  motionPreference.value = 'system';
  osReducesMotion.value = false;
  animationSpeed.value = 0;
  document.documentElement.removeAttribute('data-motion');
});

describe('reducedMotion', () => {
  const cases = [
    ['system', false, false],
    ['system', true, true],
    ['reduce', false, true],
    ['reduce', true, true],
    ['full', false, false],
    ['full', true, false],
  ] as const;

  for (const [pref, os, expected] of cases) {
    it(`is ${expected} for ${pref} with the OS switch ${os ? 'on' : 'off'}`, () => {
      motionPreference.value = pref;
      osReducesMotion.value = os;
      expect(reducedMotion.value).toBe(expected);
      expect(isReducedMotion()).toBe(expected);
    });
  }

  it('follows an OS flip live while the preference is system', () => {
    const seen: boolean[] = [];
    const stop = effect(() => { seen.push(reducedMotion.value); });
    osReducesMotion.value = true;
    osReducesMotion.value = false;
    stop();
    expect(seen).toEqual([false, true, false]);
  });
});

describe('the data-motion attribute', () => {
  it('is published from the resolved value and tracks every change', () => {
    const stop = installMotionAttribute();
    const attr = () => document.documentElement.getAttribute('data-motion');
    expect(attr()).toBe('full');

    motionPreference.value = 'reduce';
    expect(attr()).toBe('reduce');

    // `full` beats the OS, which is the case a media query cannot express.
    motionPreference.value = 'full';
    osReducesMotion.value = true;
    expect(attr()).toBe('full');

    motionPreference.value = 'system';
    expect(attr()).toBe('reduce');
    stop();
  });
});

describe('the duration scale under reduced motion', () => {
  it('collapses whatever the Animation speed slider says', () => {
    animationSpeed.value = -10;
    expect(durationScale.value).toBeCloseTo(10, 10);

    motionPreference.value = 'reduce';
    expect(durationScale.value).toBe(REDUCED_MOTION_DURATION_SCALE);

    motionPreference.value = 'full';
    osReducesMotion.value = true;
    expect(durationScale.value).toBeCloseTo(10, 10);
  });
});
