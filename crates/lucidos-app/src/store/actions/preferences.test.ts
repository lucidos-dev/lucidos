import { describe, it, expect, beforeEach } from 'vitest';
import { preferences } from '../store';
import { currentTheme } from './preferences';

describe('currentTheme — localStorage fallback', () => {
  beforeEach(() => {
    localStorage.clear();
    preferences.value = { status: 'not-loaded' };
  });

  it('returns localStorage theme when backend has no theme preference', () => {
    // User set light mode → saved in localStorage + backend
    // Backend lost the preference (device_id change, save failure, etc.)
    localStorage.setItem('lucidos-theme', 'light');
    preferences.value = { status: 'loaded', data: { 'font-family': 'monospace' } };

    // currentTheme() must respect localStorage, not default to 'dark'
    expect(currentTheme()).toBe('light');
  });

  it('returns backend theme when backend has theme preference', () => {
    localStorage.setItem('lucidos-theme', 'light');
    preferences.value = { status: 'loaded', data: { theme: 'dark' } };

    // Backend is source of truth when it has a value
    expect(currentTheme()).toBe('dark');
  });

  it('returns localStorage theme when preferences not yet loaded', () => {
    localStorage.setItem('lucidos-theme', 'light');
    preferences.value = { status: 'loading' };

    expect(currentTheme()).toBe('light');
  });

  it('returns dark as final fallback when nothing is set', () => {
    preferences.value = { status: 'loaded', data: {} };

    expect(currentTheme()).toBe('dark');
  });

  it('returns system from localStorage when backend has no theme', () => {
    localStorage.setItem('lucidos-theme', 'system');
    preferences.value = { status: 'loaded', data: {} };

    expect(currentTheme()).toBe('system');
  });

  it('returns localStorage theme when preferences failed to load', () => {
    localStorage.setItem('lucidos-theme', 'light');
    preferences.value = { status: 'failed', error: 'network error' };

    expect(currentTheme()).toBe('light');
  });

  it('skips invalid backend value and falls back to localStorage', () => {
    localStorage.setItem('lucidos-theme', 'light');
    preferences.value = { status: 'loaded', data: { theme: 'garbage' } };

    expect(currentTheme()).toBe('light');
  });

  it('ignores invalid localStorage values', () => {
    localStorage.setItem('lucidos-theme', 'purple');
    preferences.value = { status: 'loaded', data: {} };

    expect(currentTheme()).toBe('dark');
  });
});
