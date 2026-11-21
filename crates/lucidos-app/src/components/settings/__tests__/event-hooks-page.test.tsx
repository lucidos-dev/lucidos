// @vitest-environment jsdom
/** Settings › Event Hooks: each row names where its hook stands, and offers
 *  only the verbs the engine would accept. */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import type { EventHook } from '../../../api/client';

const api = vi.hoisted(() => ({
  resolve: (_: EventHook[]) => {},
  arm: vi.fn((_: EventHook) => Promise.resolve()),
  disarm: vi.fn((_: EventHook) => Promise.resolve()),
  remove: vi.fn((_: EventHook) => Promise.resolve()),
}));

vi.mock('../../../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../api/client')>()),
  fetchEventHooks: () => new Promise<EventHook[]>((r) => { api.resolve = r; }),
  armEventHook: api.arm,
  disarmEventHook: api.disarm,
  removeEventHook: api.remove,
}));

import { EventHooksPage, hookState, hookVerbs, staleArmingLine } from '../EventHooksPage';
import { SPINNER_DELAY_MS } from '../../../hooks/useDelayedLoading';
import { confirmState } from '../../../store/store';

const DEFINITION = { name: 'Deploy guard', event_type: 'ChangeApplied', script: 'check.sh' };
const ARMING = {
  slug: 'deploy-guard',
  event_type: 'ChangeApplied',
  content_hash: 'abc',
  armed_at: '2026-11-19T09:00:00Z',
};

function valid(over: Partial<EventHook> = {}): EventHook {
  return {
    status: 'valid',
    slug: 'deploy-guard',
    event_kind: 'system',
    definition: DEFINITION,
    arm_state: 'disarmed',
    armed: null,
    content_hash: 'abc',
    arm_refusal: null,
    remove_refusal: null,
    ...over,
  } as EventHook;
}

function missing(): EventHook {
  return {
    status: 'missing',
    slug: 'deploy-guard',
    arm_state: 'changed',
    armed: ARMING,
    arm_refusal: 'Its folder is gone from data/hooks/.',
    remove_refusal: null,
  };
}

describe('a row names its state and its verbs', () => {
  it('offers Arm to a disarmed hook, and Disarm once armed', () => {
    expect(hookState(valid())).toBe('Disarmed');
    expect(hookVerbs(valid())).toEqual({ arm: true, disarm: false });
    const armed = valid({ arm_state: 'armed', armed: ARMING });
    expect(hookState(armed)).toBe('Armed');
    expect(hookVerbs(armed)).toEqual({ arm: false, disarm: true });
    expect(staleArmingLine(armed)).toBeNull();
  });

  it('offers both to a changed hook, and says its events are refused', () => {
    const changed = valid({ arm_state: 'changed', armed: ARMING, content_hash: 'def' });
    expect(hookState(changed)).toBe('Changed');
    expect(hookVerbs(changed)).toEqual({ arm: true, disarm: true });
    expect(staleArmingLine(changed)).toContain('refuses its events');
  });

  it('calls a gone folder Missing, which cannot be armed again', () => {
    expect(hookState(missing())).toBe('Missing');
    expect(hookVerbs(missing())).toEqual({ arm: false, disarm: true });
    expect(staleArmingLine(missing())).toContain('folder is gone');
  });
});

describe('the page', () => {
  let host: HTMLDivElement;

  beforeEach(() => {
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
    api.arm.mockClear();
    api.remove.mockClear();
    confirmState.value = { visible: false, message: '', okLabel: '' };
    host = document.createElement('div');
    document.body.appendChild(host);
    act(() => { render(<EventHooksPage />, host); });
  });

  afterEach(() => {
    render(null, host);
    host.remove();
    vi.useRealTimers();
  });

  async function land(rows: EventHook[]) {
    await act(async () => { api.resolve(rows); });
  }

  function button(label: string): HTMLButtonElement | undefined {
    return [...host.querySelectorAll<HTMLButtonElement>('.loading-fade-content button')]
      .find((b) => b.textContent === label);
  }

  it('draws hook rows as the skeleton once the delay gate opens', () => {
    expect(host.querySelector('.sk-bar')).toBeNull();
    act(() => { vi.advanceTimersByTime(SPINNER_DELAY_MS); });
    expect(host.querySelector('.loading-fade-skeleton .list-row .title .sk-bar')).not.toBeNull();
    expect(host.textContent).not.toContain('Loading');
  });

  it('says so when there are no hooks', async () => {
    await land([]);
    expect(host.querySelector('.empty-state')?.textContent).toBe('No event hooks yet.');
  });

  it('disables Arm beside the reason it cannot be armed', async () => {
    const why = 'Lucidos does not ask hooks about ChangeApplied events yet, so this hook could never run.';
    await land([valid({ arm_refusal: why })]);
    expect(button('Arm')?.disabled).toBe(true);
    expect(host.textContent).toContain(why);
  });

  it('lists an invalid hook by its folder, with its problems', async () => {
    await land([{
      status: 'invalid',
      slug: 'My Hook',
      definition: null,
      problems: ['hook.toml is missing.'],
      arm_state: 'disarmed',
      armed: null,
      arm_refusal: 'Fix the problems listed first.',
      remove_refusal: null,
    }]);
    expect(host.querySelector('.loading-fade-content .title')?.textContent).toBe('My Hook');
    expect(host.querySelector('.error-text')?.textContent).toBe('hook.toml is missing.');
    expect(button('Remove')).toBeDefined();
  });

  it('arms only after the owner confirms', async () => {
    await land([valid()]);
    act(() => { button('Arm')?.click(); });
    expect(confirmState.value.message).toContain('Arm the event hook "Deploy guard"?');
    expect(confirmState.value.variant).toBe('default');
    expect(api.arm).not.toHaveBeenCalled();
    await act(async () => { confirmState.value.resolve?.(true); });
    expect(api.arm).toHaveBeenCalledWith(
      expect.objectContaining({ slug: 'deploy-guard', content_hash: 'abc' }),
    );
  });

  it('disables Remove beside the reason it cannot be removed here', async () => {
    const why = "'a`b' does not name a folder under data/hooks/. Rename or delete it outside Lucidos.";
    await land([valid({ remove_refusal: why })]);
    expect(button('Remove')?.disabled).toBe(true);
    expect(host.textContent).toContain(why);
  });

  it('asks a danger confirm before it deletes a folder', async () => {
    await land([valid()]);
    act(() => { button('Remove')?.click(); });
    expect(confirmState.value.message).toContain('data/hooks/deploy-guard/');
    expect(confirmState.value.variant).toBe('danger');
    await act(async () => { confirmState.value.resolve?.(false); });
    expect(api.remove).not.toHaveBeenCalled();
  });

  it('removes a missing hook with a plain confirm, since no folder is lost', async () => {
    await land([missing()]);
    expect(button('Arm')).toBeUndefined();
    act(() => { button('Remove')?.click(); });
    expect(confirmState.value.variant).toBe('default');
    await act(async () => { confirmState.value.resolve?.(true); });
    expect(api.remove).toHaveBeenCalledWith(expect.objectContaining({ slug: 'deploy-guard' }));
  });
});
