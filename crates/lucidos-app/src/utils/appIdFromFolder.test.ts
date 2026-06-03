import { describe, it, expect } from 'vitest';
import { appIdFromFolder } from './appIdFromFolder';

describe('appIdFromFolder', () => {
  it('extracts id from absolute workspace path', () => {
    expect(appIdFromFolder('/Users/me/workspaces/personal/data/apps/momentum-autoresearch'))
      .toBe('momentum-autoresearch');
  });

  it('handles trailing slash', () => {
    expect(appIdFromFolder('/ws/data/apps/momentum/')).toBe('momentum');
  });

  it('handles workspace-relative path', () => {
    expect(appIdFromFolder('data/apps/habit-tracker')).toBe('habit-tracker');
  });

  it('returns null for paths under data/ but not apps/', () => {
    expect(appIdFromFolder('/ws/data/artifacts/foo')).toBeNull();
    expect(appIdFromFolder('/ws/data/knowhow/bar.md')).toBeNull();
  });

  it('returns null when no segment follows apps/', () => {
    expect(appIdFromFolder('/ws/data/apps')).toBeNull();
    expect(appIdFromFolder('/ws/data/apps/')).toBeNull();
  });

  it('refuses dot segments as id', () => {
    expect(appIdFromFolder('/ws/data/apps/.')).toBeNull();
    expect(appIdFromFolder('/ws/data/apps/..')).toBeNull();
  });

  it('requires data/ immediately before apps/', () => {
    expect(appIdFromFolder('/ws/apps/foo')).toBeNull();
    expect(appIdFromFolder('/ws/repo/apps/foo')).toBeNull();
  });

  it('returns null for empty / null / undefined', () => {
    expect(appIdFromFolder(null)).toBeNull();
    expect(appIdFromFolder(undefined)).toBeNull();
    expect(appIdFromFolder('')).toBeNull();
  });

  it('takes the last data/apps/ if path is unusually nested', () => {
    expect(appIdFromFolder('/ws/data/apps/outer/data/apps/inner')).toBe('inner');
  });
});
