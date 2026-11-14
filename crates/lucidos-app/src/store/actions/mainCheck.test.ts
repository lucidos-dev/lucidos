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

  it('a linked run shows on the running check, and only there', () => {
    const link = { check_id: 'k-1', run_url: 'https://example.invalid/runs/1', branch: 'e2e/x' };
    expect(mainCheckAfter(RUNNING, 'MainCheckRunLinked', link))
      .toEqual({ ...RUNNING, run_url: 'https://example.invalid/runs/1' });
    const passed = mainCheckAfter(RUNNING, 'MainCheckPassed', { check_id: 'k-1' });
    expect(mainCheckAfter(passed, 'MainCheckRunLinked', link)).toEqual(passed);
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

describe('repair-aware snapshots', () => {
  const waiting: MainCheck = {
    state: 'red', check_id: 'failed-check', summary: 'Failed: api.',
    repair: { state: 'waiting', fix_thread_id: 'repair-owner', waiting_on: ['queued work'] },
  };
  it('keeps red and its owner while a newer check starts', () => {
    expect(mainCheckAfter(waiting, 'MainCheckStarted', { check_id: 'new-check' })).toBe(waiting);
    expect(mainCheckAfter(waiting, 'MainCheckFailed', {
      check_id: 'new-check', superseded_by_fix: 'repair-owner',
    })).toBe(waiting);
  });
  it('marks only the matching repair running', () => {
    const started = { check_id: 'new-check', fix_thread_id: 'repair-owner', started_by: 'auto' };
    expect(mainCheckAfter(waiting, 'MainCheckRepairStarted', started).repair)
      .toEqual({ state: 'running', fix_thread_id: 'repair-owner', started_by: 'auto', waiting_on: [] });
    expect(mainCheckAfter(waiting, 'MainCheckRepairStarted', { ...started, fix_thread_id: 'other' })).toBe(waiting);
  });
});
