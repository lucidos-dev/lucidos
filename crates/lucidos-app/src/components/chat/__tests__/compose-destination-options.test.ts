/** The compose destination picker's option list — the packaged-build gate on
 *  "Lucidos source" plus the loading / failed / unavailable rows. Pure builder,
 *  so no component render or signal mocking needed. */

import { describe, it, expect } from 'vitest';
import { buildDestinationOptions, type DestinationOptionInputs } from '../composeDestinationOptions';
import type { ComposeDestination } from '../../../store/composeDestination';

const AGENT: ComposeDestination = { kind: 'lucidos-agent' };

function inputs(over: Partial<DestinationOptionInputs> = {}): DestinationOptionInputs {
  return {
    dest: AGENT,
    apps: [],
    externalRepos: [],
    packaged: false,
    listsPending: false,
    reposError: null,
    appsError: null,
    ...over,
  };
}

const values = (opts: { value: string }[]) => opts.map(o => o.value);

describe('buildDestinationOptions — Lucidos source packaged gate', () => {
  it('offers Lucidos source on a dev build (source checkout present)', () => {
    const opts = buildDestinationOptions(inputs({ packaged: false }));
    expect(values(opts)).toContain('code:lucidos');
    expect(opts.find(o => o.value === 'code:lucidos')?.label).toBe('Lucidos source');
  });

  it('hides Lucidos source on a packaged build (no source checkout to edit)', () => {
    const opts = buildDestinationOptions(inputs({ packaged: true }));
    expect(values(opts)).not.toContain('code:lucidos');
  });

  it('still offers apps + external repos (and the register action) when packaged', () => {
    const opts = buildDestinationOptions(inputs({
      packaged: true,
      apps: [{ id: 'habit-tracker', name: 'Habit Tracker' }],
      externalRepos: [{ id: 'r1', name: 'my-project' }],
    }));
    expect(values(opts)).toContain('app:habit-tracker');
    expect(values(opts)).toContain('repo:r1');
    expect(values(opts)).toContain('__hdr-coding');
    expect(values(opts)).toContain('__register-repo');
    expect(values(opts)).not.toContain('code:lucidos');
  });
});

describe('buildDestinationOptions — loading / error / unavailable rows', () => {
  it('adds a loading row while lists pend, and no unavailable row yet', () => {
    const opts = buildDestinationOptions(inputs({
      listsPending: true,
      dest: { kind: 'coding', scope: { kind: 'app', appId: 'gone' } },
    }));
    expect(values(opts)).toContain('__loading');
    expect(opts.some(o => o.label.endsWith('· unavailable'))).toBe(false);
  });

  it('surfaces failed repos / apps loads as distinct danger rows', () => {
    const opts = buildDestinationOptions(inputs({ reposError: 'boom-r', appsError: 'boom-a' }));
    const repoErr = opts.find(o => o.value === '__repos-error');
    const appErr = opts.find(o => o.value === '__apps-error');
    expect(repoErr?.description).toBe('boom-r');
    expect(repoErr?.danger).toBe(true);
    expect(appErr?.description).toBe('boom-a');
    expect(appErr?.danger).toBe(true);
  });

  it('marks a restored target the lists no longer contain as unavailable', () => {
    const opts = buildDestinationOptions(inputs({
      dest: { kind: 'coding', scope: { kind: 'app', appId: 'ghost' } },
    }));
    const row = opts.find(o => o.value === 'app:ghost');
    expect(row?.label).toBe('ghost · unavailable');
    expect(row?.disabled).toBe(true);
    expect(row?.danger).toBe(true);
  });

  it('names a hidden Lucidos-source target clearly when restored on a packaged build', () => {
    const opts = buildDestinationOptions(inputs({
      packaged: true,
      dest: { kind: 'coding', scope: { kind: 'lucidos' } },
    }));
    // Not offered as a selectable option...
    expect(opts.some(o => o.value === 'code:lucidos' && !o.disabled)).toBe(false);
    // ...but a stale device-global selection is surfaced as unavailable, named.
    const row = opts.find(o => o.value === 'code:lucidos');
    expect(row?.label).toBe('Lucidos source · unavailable');
    expect(row?.danger).toBe(true);
  });
});
