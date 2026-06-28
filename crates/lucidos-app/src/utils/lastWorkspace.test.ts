import { describe, it, expect, beforeEach } from 'vitest';
import {
  LAST_WORKSPACE_KEY,
  LAST_WORKSPACE_COUNT_KEY,
  rememberLastWorkspace,
  recallLastWorkspace,
  forgetLastWorkspace,
  rememberLastWorkspaceCount,
  recallLastWorkspaceCount,
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

describe('lastWorkspace count memory (skeleton sizing)', () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it('returns null when nothing is recorded', () => {
    expect(recallLastWorkspaceCount()).toBeNull();
  });

  it('round-trips the count at the raw device-global key', () => {
    rememberLastWorkspaceCount(4);
    expect(recallLastWorkspaceCount()).toBe(4);
    expect(localStorage.getItem(LAST_WORKSPACE_COUNT_KEY)).toBe('4');
  });

  it('treats a zero count as nothing recorded (caller defaults)', () => {
    rememberLastWorkspaceCount(0);
    expect(recallLastWorkspaceCount()).toBeNull();
  });

  it('clamps an implausibly large count to the skeleton max', () => {
    rememberLastWorkspaceCount(500);
    expect(recallLastWorkspaceCount()).toBe(20);
  });

  it('returns null for a corrupt stored value', () => {
    localStorage.setItem(LAST_WORKSPACE_COUNT_KEY, 'not-a-number');
    expect(recallLastWorkspaceCount()).toBeNull();
  });
});
