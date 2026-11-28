import { describe, it, expect } from 'vitest';
import {
  blockIsOpen,
  overrideIsSettled,
  providerBlockLoaded,
  providerState,
  switchAction,
  systemOneBlockLoaded,
  systemOneProviderState,
  type ProviderEnablementInput,
} from '../providerEnablement';

const base: ProviderEnablementInput = {
  installed: [],
  switchedOff: false,
};

describe('providerState', () => {
  it('is on when the engine is serving the provider', () => {
    expect(providerState('openai', { ...base, installed: ['openai'] })).toBe('on');
    expect(providerState('vertex', { ...base, installed: ['vertex'] })).toBe('on');
  });

  it('tells a switched-off provider from one never set up', () => {
    expect(providerState('openai', { ...base, switchedOff: true })).toBe('switched-off');
    expect(providerState('openai', base)).toBe('not-set-up');
  });

  it('reports a stored key as off once the switch says off', () => {
    expect(providerState('anthropic', { ...base, switchedOff: true })).toBe('switched-off');
  });
});

describe('systemOneProviderState', () => {
  it('is never set up until a credential is stored', () => {
    expect(systemOneProviderState({ configured: false, switchedOff: false }))
      .toBe('not-set-up');
    expect(systemOneProviderState({ configured: false, switchedOff: true }))
      .toBe('not-set-up');
  });

  it('is on once a credential is stored and nothing switched it off', () => {
    expect(systemOneProviderState({ configured: true, switchedOff: false })).toBe('on');
  });

  it('tells a parked provider from one never set up', () => {
    expect(systemOneProviderState({ configured: true, switchedOff: true }))
      .toBe('switched-off');
  });
});

describe('systemOneBlockLoaded', () => {
  it('needs both the credentials and the preferences', () => {
    expect(systemOneBlockLoaded(true, true)).toBe(true);
    expect(systemOneBlockLoaded(false, true)).toBe(false);
    expect(systemOneBlockLoaded(true, false)).toBe(false);
  });
});

describe('providerBlockLoaded', () => {
  it('needs both the provider list and the preferences', () => {
    expect(providerBlockLoaded([], true)).toBe(true);
    expect(providerBlockLoaded(['openai'], true)).toBe(true);
    expect(providerBlockLoaded([], false)).toBe(false);
  });

  it('treats an unknown provider list as not loaded, never as nothing', () => {
    expect(providerBlockLoaded(null, true)).toBe(false);
  });
});

describe('switchAction', () => {
  it('writes the preference only where there is something to switch', () => {
    expect(switchAction('on', false)).toBe('disable');
    expect(switchAction('switched-off', true)).toBe('enable');
  });

  it('only discloses for a provider that was never set up', () => {
    expect(switchAction('not-set-up', true)).toBe('expand');
    expect(switchAction('not-set-up', false)).toBe('collapse');
  });

  it('collapses a switched-off block without writing again', () => {
    expect(switchAction('switched-off', false)).toBe('collapse');
  });
});

describe('blockIsOpen', () => {
  it('follows the engine when the user has pressed nothing', () => {
    expect(blockIsOpen('on', null)).toBe(true);
    expect(blockIsOpen('not-set-up', null)).toBe(false);
    expect(blockIsOpen('switched-off', null)).toBe(false);
  });

  it('holds a press the engine has not caught up with', () => {
    expect(blockIsOpen('not-set-up', true)).toBe(true);
    expect(blockIsOpen('on', false)).toBe(false);
  });
});

describe('overrideIsSettled', () => {
  it('is false while nothing is held', () => {
    expect(overrideIsSettled('on', null)).toBe(false);
    expect(overrideIsSettled('not-set-up', null)).toBe(false);
  });

  it('is false while the engine still disagrees', () => {
    expect(overrideIsSettled('on', false)).toBe(false);
    expect(overrideIsSettled('not-set-up', true)).toBe(false);
  });

  it('is true once reality matches the press', () => {
    expect(overrideIsSettled('on', true)).toBe(true);
    expect(overrideIsSettled('switched-off', false)).toBe(true);
    expect(overrideIsSettled('not-set-up', false)).toBe(true);
  });
});
