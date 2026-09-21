import { describe, it, expect } from 'vitest';
import { restartConfirmCopy } from './restartConfirmCopy';
import type { PendingCommits } from '../api/client';
import type { RestartGroup } from './store';

const COMMITS: PendingCommits = {
  total: 5,
  groups: [
    { kind: 'new', total: 1, descriptions: ['ui: a modal that says what it brings'] },
    { kind: 'fixed', total: 2, descriptions: ['composer: the close survives', 'todo: the guard reads once'] },
    { kind: 'housekeeping', total: 2, descriptions: [] },
  ],
};

const APPLIED: RestartGroup[] = [
  { threadId: 't1', threadTitle: 'Make the badge honest', commits: ['fix: the badge'] },
];

describe('the plain-restart shape', () => {
  it('asks its short question and lists the applied changes', () => {
    const copy = restartConfirmCopy(false, null, APPLIED);
    expect(copy.title).toBeUndefined();
    expect(copy.message).toBe('Restart engine?');
    expect(copy.okLabel).toBe('Restart');
    expect(copy.cancelLabel).toBeUndefined();
    expect(copy.details).toEqual({
      intro: 'These changes will be applied:',
      groups: [{ header: 'Make the badge honest', items: ['fix: the badge'] }],
    });
  });

  it('never describes the version, even when the engine listed a range', () => {
    // A plain restart brings nothing new, so a commit range would be a claim
    // about a switch that is not on offer.
    const copy = restartConfirmCopy(false, COMMITS, []);
    expect(copy.details).toBeUndefined();
  });

  it('shows no list at all when nothing has been applied', () => {
    expect(restartConfirmCopy(false, null, []).details).toBeUndefined();
  });
});

describe('the new-version shape', () => {
  it('keeps the canonical name of the action and offers Later', () => {
    const copy = restartConfirmCopy(true, COMMITS, []);
    expect(copy.title).toBe('New version available');
    expect(copy.okLabel).toBe('Switch to new version');
    expect(copy.cancelLabel).toBe('Later');
  });

  it('describes the range, and folds housekeeping into the count', () => {
    const copy = restartConfirmCopy(true, COMMITS, []);
    expect(copy.details).toEqual({
      intro: '5 commits since your running version, including 2 housekeeping commits (docs, tests, chores).',
      groups: [
        { header: 'New', items: ['ui: a modal that says what it brings'] },
        { header: 'Fixed', items: ['composer: the close survives', 'todo: the guard reads once'] },
      ],
    });
  });

  it('names the tail the engine capped off', () => {
    const capped: PendingCommits = {
      total: 9,
      groups: [{ kind: 'new', total: 9, descriptions: ['one', 'two'] }],
    };
    const copy = restartConfirmCopy(true, capped, []);
    expect(copy.details?.groups[0].items).toEqual(['one', 'two', 'and 7 more']);
  });

  it('counts one commit in the singular', () => {
    const one: PendingCommits = {
      total: 1,
      groups: [{ kind: 'fixed', total: 1, descriptions: ['the one thing'] }],
    };
    expect(restartConfirmCopy(true, one, []).details?.intro).toBe('1 commit since your running version.');
  });

  it('states an all-housekeeping range once rather than twice', () => {
    // "12 commits, including 12 housekeeping commits" is true and reads like a
    // mistake, so the count IS the sentence there.
    const chores: PendingCommits = {
      total: 3,
      groups: [{ kind: 'housekeeping', total: 3, descriptions: [] }],
    };
    expect(restartConfirmCopy(true, chores, []).details).toEqual({
      intro: '3 housekeeping commits (docs, tests, chores) since your running version.',
      groups: [],
    });
  });

  it('falls back to the applied changes when git could not answer', () => {
    // `null` is UNKNOWN. A packaged build reports it on every poll, and its
    // applied changes are the only account of what the restart activates.
    const copy = restartConfirmCopy(true, null, APPLIED);
    expect(copy.details?.intro).toBe('These changes will be applied:');
  });

  it('reports a genuinely empty range as no list, never as a zero', () => {
    // `{ total: 0 }` is a real answer, unlike `null`: the range IS empty, so
    // there is nothing to list and nothing to fall back to.
    const empty: PendingCommits = { total: 0, groups: [] };
    expect(restartConfirmCopy(true, empty, APPLIED).details?.intro).toBe('These changes will be applied:');
    expect(restartConfirmCopy(true, empty, []).details).toBeUndefined();
  });
});
