import { describe, it, expect, beforeEach } from 'vitest';
import {
  LAST_WORKSPACE_KEY,
  rememberLastWorkspace,
  recallLastWorkspace,
  forgetLastWorkspace,
} from './lastWorkspace';

describe('lastWorkspace memory', () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it('returns null when nothing is recorded', () => {
    expect(recallLastWorkspace()).toBeNull();
  });

  it('round-trips the recorded workspace slug at the raw device-global key', () => {
    rememberLastWorkspace('alpha');
    expect(recallLastWorkspace()).toBe('alpha');
    expect(localStorage.getItem(LAST_WORKSPACE_KEY)).toBe('alpha');
  });

  it('overwrites a previously recorded workspace', () => {
    rememberLastWorkspace('alpha');
    rememberLastWorkspace('beta');
    expect(recallLastWorkspace()).toBe('beta');
  });

  it('forgets the recorded workspace', () => {
    rememberLastWorkspace('alpha');
    forgetLastWorkspace();
    expect(recallLastWorkspace()).toBeNull();
  });
});
