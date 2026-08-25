/**
 * The provider switch's state machine.
 *
 * Three states and four actions, all pure, because every one of them is a
 * decision the Providers page must not make twice: what the toggle shows, what
 * pressing it writes, and whether the config rows are on screen.
 */
import { describe, it, expect } from 'vitest';
import {
  blockIsOpen,
  overrideIsSettled,
  providerBlockLoaded,
  providerState,
  switchAction,
  type ProviderEnablementInput,
} from '../providerEnablement';

/** Nothing configured, nothing switched, `/health` has answered. */
const base: ProviderEnablementInput = {
  installed: [],
  switchedOff: false,
};

describe('providerState', () => {
  it('is on when the engine is serving the provider', () => {
    expect(providerState('openai', { ...base, installed: ['openai'] })).toBe('on');
    // Including a provider the engine got from the launch environment: there is
    // no credential here to read, and the switch still turns it off.
    expect(providerState('vertex', { ...base, installed: ['vertex'] })).toBe('on');
  });

  it('tells a switched-off provider from one never set up', () => {
    expect(providerState('openai', { ...base, switchedOff: true })).toBe('switched-off');
    expect(providerState('openai', base)).toBe('not-set-up');
  });

  it('reports a stored key as off once the switch says off', () => {
    // The key is still there. That is the point of the switch, and it must not
    // make the provider read as running.
    expect(providerState('anthropic', { ...base, switchedOff: true })).toBe('switched-off');
  });
});

describe('providerBlockLoaded', () => {
  it('needs both the provider list and the preferences', () => {
    expect(providerBlockLoaded([], true)).toBe(true);
    expect(providerBlockLoaded(['openai'], true)).toBe(true);
    expect(providerBlockLoaded([], false)).toBe(false);
  });

  it('treats an unknown provider list as not loaded, never as nothing', () => {
    // A provider the launch environment configured stores nothing here, so
    // guessing before /health answers draws a running provider as never set up.
    // A press against that guess writes nothing and is silently dropped.
    expect(providerBlockLoaded(null, true)).toBe(false);
  });
});

describe('switchAction', () => {
  it('writes the preference only where there is something to switch', () => {
    expect(switchAction('on', false)).toBe('disable');
    expect(switchAction('switched-off', true)).toBe('enable');
  });

  it('only discloses for a provider that was never set up', () => {
    // No write. A stored `false` here would veto a key added later by env var,
    // over a press the user meant as "show me the fields".
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
    // Opening a provider with no key installs nothing, so the derived state
    // would hide the fields the user just opened to paste a key into.
    expect(blockIsOpen('not-set-up', true)).toBe(true);
    // Switching one off is a rebuild plus a probe away. Derived alone, the
    // switch springs back on for the length of that round trip.
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
    // Dropping the held press here is what lets the engine correct one that
    // did not take, e.g. a saved key the provider rejected.
    expect(overrideIsSettled('on', true)).toBe(true);
    expect(overrideIsSettled('switched-off', false)).toBe(true);
    expect(overrideIsSettled('not-set-up', false)).toBe(true);
  });
});
