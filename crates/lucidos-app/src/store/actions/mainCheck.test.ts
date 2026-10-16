import { describe, it, expect, beforeEach } from 'vitest';
import type { MainCheck } from '../../api/client';
import { IDLE_MAIN_CHECK, mainCheck, mainCheckClosing } from '../store';
import { mainCheckAfter, setMainCheckFromServer } from './mainCheck';

const RUNNING: MainCheck = { state: 'running', check_id: 'k-1', from_sha: 'aaa', to_sha: 'bbb' };

describe('the main check frames, as pure steps', () => {
  it('a start runs the newest check, whatever ran before', () => {
    expect(mainCheckAfter(IDLE_MAIN_CHECK, 'MainCheckStarted', { check_id: 'k-1', from_sha: 'aaa', to_sha: 'bbb' }))
      .toEqual(RUNNING);
  });

  it('a pass keeps the range it cleared', () => {
    expect(mainCheckAfter(RUNNING, 'MainCheckPassed', { check_id: 'k-1' }))
      .toEqual({ state: 'passed', check_id: 'k-1', from_sha: 'aaa', to_sha: 'bbb' });
  });

  it('a failure goes red with its summary, run and fix thread', () => {
    expect(mainCheckAfter(RUNNING, 'MainCheckFailed', {
      check_id: 'k-1',
      summary: 'Failed: engine.',
      tail: 'test result: FAILED',
      run_url: 'https://example.invalid/runs/1',
      fix_thread_id: 'fix-t',
    })).toEqual({
      state: 'red',
      check_id: 'k-1',
      from_sha: 'aaa',
      to_sha: 'bbb',
      run_url: 'https://example.invalid/runs/1',
      fix_thread_id: 'fix-t',
      summary: 'Failed: engine.',
    });
  });

  it('no verdict leaves the check idle, and a closed red too', () => {
    expect(mainCheckAfter(RUNNING, 'MainCheckAbandoned', { check_id: 'k-1', reason: 'GitHub is down' }))
      .toEqual(IDLE_MAIN_CHECK);
    const red = mainCheckAfter(RUNNING, 'MainCheckFailed', { check_id: 'k-1', summary: 's', fix_thread_id: 'f' });
    expect(mainCheckAfter(red, 'MainCheckRedClosed', { check_id: 'k-1', reason: 'fix_landed' }))
      .toEqual(IDLE_MAIN_CHECK);
  });

  it('a frame about another check changes nothing', () => {
    expect(mainCheckAfter(RUNNING, 'MainCheckPassed', { check_id: 'k-other' })).toBe(RUNNING);
    expect(mainCheckAfter(RUNNING, 'MainCheckRedClosed', { check_id: 'k-1' })).toBe(RUNNING);
  });
});

describe('setMainCheckFromServer', () => {
  beforeEach(() => {
    mainCheck.value = IDLE_MAIN_CHECK;
    mainCheckClosing.value = null;
  });

  it('keeps a pressed Close while its red is open, and drops it once it closed', () => {
    mainCheckClosing.value = 'k-1';
    setMainCheckFromServer({ state: 'red', check_id: 'k-1', summary: 's' });
    expect(mainCheckClosing.value).toBe('k-1');
    setMainCheckFromServer({ state: 'idle' });
    expect(mainCheckClosing.value).toBeNull();
  });

  it('reads an older engine with no main check as idle', () => {
    mainCheck.value = RUNNING;
    setMainCheckFromServer(undefined);
    expect(mainCheck.value).toEqual(IDLE_MAIN_CHECK);
  });
});
