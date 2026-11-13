// @vitest-environment jsdom
import { afterEach, expect, it } from 'vitest';
import { isReducedMotion, scaledDurationMs } from './motion';

afterEach(() => {
  document.documentElement.removeAttribute('data-motion');
  document.documentElement.style.removeProperty('--duration-scale');
});

it('uses the frame appearance and follows updates without browser storage', () => {
  expect(isReducedMotion()).toBe(false);
  expect(scaledDurationMs(100)).toBe(100);
  document.documentElement.style.setProperty('--duration-scale', '2');
  expect(scaledDurationMs(100)).toBe(200);
  document.documentElement.setAttribute('data-motion', 'reduce');
  expect(isReducedMotion()).toBe(true);
  expect(scaledDurationMs(100)).toBe(0.1);
});
