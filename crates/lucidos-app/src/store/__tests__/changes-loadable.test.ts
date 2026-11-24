import { describe, it, expect } from 'vitest';
import { changes, appliedChanges } from '../store';

describe('changes / appliedChanges are Loadable<Change[]>', () => {
  it('changes can represent a load failure', () => {
    changes.value = { status: 'failed', error: 'DB error', httpCode: 500 };
    expect(changes.value.status).toBe('failed');
    if (changes.value.status === 'failed') {
      expect(changes.value.error).toBe('DB error');
      expect(changes.value.httpCode).toBe(500);
    }
  });

  it('appliedChanges can represent a load failure', () => {
    appliedChanges.value = { status: 'failed', error: 'DB error' };
    expect(appliedChanges.value.status).toBe('failed');
  });

  it('both start in not-loaded state, not as empty arrays', () => {
    changes.value = { status: 'not-loaded' };
    appliedChanges.value = { status: 'not-loaded' };
    expect(changes.value.status).toBe('not-loaded');
    expect(appliedChanges.value.status).toBe('not-loaded');
  });

  it('loaded state carries Change[] data', () => {
    changes.value = { status: 'loaded', data: [] };
    expect(changes.value.status).toBe('loaded');
    if (changes.value.status === 'loaded') {
      expect(changes.value.data).toEqual([]);
    }
  });
});
