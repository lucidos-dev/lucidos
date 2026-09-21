import { describe, it, expect } from 'vitest';
import {
  CHAT,
  JEV,
  JEV_MODEL_LABEL,
  JEV_MODEL_VALUE,
  JUDGMENT_MODEL_KEY,
  JUDGMENT_PREFERENCE_KEY,
  JUDGMENT_REASONING_KEY,
  jevConsumers,
  jevRowOffered,
  judgmentModelChoices,
  judgmentPickWrites,
  judgmentSelectedEffort,
  judgmentSelectedModel,
  judgmentSelectionCaveat,
  typeSafeKeyStored,
  typeSafeSwitchedOff,
  wantsJev,
} from './judgmentBackend';
import type { ModelChoice } from '../../store/modelSelection';
import type { AuthType, CredentialInfo, Loadable } from '../../store/types';

function cred(service_name: string, auth_type: AuthType = 'api_key'): CredentialInfo {
  return {
    id: 'c1',
    service_name,
    base_urls: ['https://api.example.test'],
    auth_type,
    auth_header: 'Authorization',
    created_at: '2026-09-19T00:00:00Z',
  } as CredentialInfo;
}

function loaded(data: CredentialInfo[]): Loadable<CredentialInfo[]> {
  return { status: 'loaded', data };
}

describe('wantsJev', () => {
  /** The same two words the engine's `wants_jev` matches. Drift here shows a
   *  switch position the engine does not act on. */
  it('accepts only the word jev, forgiving case and space', () => {
    expect(wantsJev('jev')).toBe(true);
    expect(wantsJev('  JEV  ')).toBe(true);
    expect(wantsJev('Jev')).toBe(true);
  });

  it('reads every other value as chat', () => {
    expect(wantsJev('chat')).toBe(false);
    expect(wantsJev('')).toBe(false);
    expect(wantsJev('   ')).toBe(false);
    expect(wantsJev('jevvy')).toBe(false);
    expect(wantsJev('typesafe')).toBe(false);
    expect(wantsJev(undefined)).toBe(false);
    expect(wantsJev(null)).toBe(false);
  });
});

describe('JUDGMENT_PREFERENCE_KEY', () => {
  /** The keys the engine declares. A typo here writes a preference nothing
   *  reads, which looks exactly like a control that does nothing. */
  it('names the two engine preference keys', () => {
    expect(JUDGMENT_PREFERENCE_KEY['command-guard']).toBe('judgment_command_guard');
    expect(JUDGMENT_PREFERENCE_KEY['query-classification'])
      .toBe('judgment_query_classification');
  });

  /** Each site's chat path runs its own *model selection*. Sharing one pair
   *  across both would move the command guard's judge from a Models page. */
  it('pairs each site with its own model and reasoning keys', () => {
    expect(JUDGMENT_MODEL_KEY['command-guard']).toBe('model_command_judge');
    expect(JUDGMENT_REASONING_KEY['command-guard']).toBe('reasoning_command_judge');
    expect(JUDGMENT_MODEL_KEY['query-classification']).toBe('model_query_classification');
    expect(JUDGMENT_REASONING_KEY['query-classification'])
      .toBe('reasoning_query_classification');
  });
});

describe('typeSafeKeyStored', () => {
  it('sees a stored typesafe credential', () => {
    expect(typeSafeKeyStored(loaded([cred('typesafe')]))).toBe(true);
  });

  it('is false for another service, or while credentials are loading', () => {
    expect(typeSafeKeyStored(loaded([cred('openai')]))).toBe(false);
    expect(typeSafeKeyStored(loaded([]))).toBe(false);
    expect(typeSafeKeyStored({ status: 'loading' })).toBe(false);
  });

  /** An OAuth client registration may share the name. Counting one would
   *  promise a key the engine never reads. */
  it('ignores an oauth_client row of the same name', () => {
    expect(typeSafeKeyStored(loaded([cred('typesafe', 'oauth_client')]))).toBe(false);
  });
});

describe('typeSafeSwitchedOff', () => {
  const stored = (value?: string) => typeSafeSwitchedOff({
    status: 'loaded',
    data: value === undefined ? {} : { provider_enabled_typesafe: value },
  });

  /** Absent means on, the rule every `provider_enabled_*` key follows. Reading
   *  unset as off would switch Jev dark on every workspace already using it. */
  it('is false while unset, set to true, or still loading', () => {
    expect(stored()).toBe(false);
    expect(stored('true')).toBe(false);
    expect(typeSafeSwitchedOff({ status: 'loading' })).toBe(false);
  });

  it('is true once the switch says false', () => {
    expect(stored('false')).toBe(true);
  });

  /** The engine's `reads_as_false` takes four spellings, and nothing corrects
   *  this side: TypeSafe has no `/health` row. A value only the engine calls
   *  off would draw the row live while it ran chat. */
  it('reads every spelling the engine reads as off', () => {
    for (const value of ['0', 'no', 'off', 'OFF', '  false  ']) {
      expect(stored(value), `${value} must read as off`).toBe(true);
    }
  });

  it('still reads an unrecognized word as on, like the engine', () => {
    expect(stored('nope')).toBe(false);
    expect(stored('')).toBe(false);
  });
});

describe('jevConsumers', () => {
  const none: Loadable<Record<string, string>> = { status: 'loaded', data: {} };

  /** The tool has no preference of its own: a stored key IS its condition
   *  (ADR 0223). So it is listed where no classification has moved at all. */
  it('lists the judge tool on a key alone, ahead of any site', () => {
    expect(jevConsumers(none, true)).toEqual(['The agent’s judge tool']);
  });

  /** Without a key the fold is open only because the user pressed to peek at
   *  the fields. Nothing is running, so nothing may be named. */
  it('names nothing at all with no key stored', () => {
    expect(jevConsumers(none, false)).toEqual([]);
  });

  it('adds each site that has been switched over', () => {
    const both: Loadable<Record<string, string>> = {
      status: 'loaded',
      data: { judgment_command_guard: JEV, judgment_query_classification: JEV },
    };
    expect(jevConsumers(both, true)).toEqual([
      'The agent’s judge tool',
      'Command guard',
      'Query classification',
    ]);
  });
});

describe('jevRowOffered', () => {
  /** ADR 0220's no-change promise, seen from the picker. A workspace with no
   *  TypeSafe key must see exactly the models it saw before Jev existed. */
  it('is absent with no key stored', () => {
    expect(jevRowOffered({ onJev: false, keyStored: false, typeSafeOff: false })).toBe(false);
  });

  it('appears once a key is stored and the provider is on', () => {
    expect(jevRowOffered({ onJev: false, keyStored: true, typeSafeOff: false })).toBe(true);
  });

  /** A switched-off provider offers no models, the same way an unconfigured
   *  one offers none. */
  it('is absent while TypeSafe is switched off', () => {
    expect(jevRowOffered({ onJev: false, keyStored: true, typeSafeOff: true })).toBe(false);
  });

  /** The engine falls back to the launch environment, which this page cannot
   *  see. Without the row, such a workspace renders a selection it can never
   *  change. */
  it('is offered to a site already on Jev with no stored key', () => {
    expect(jevRowOffered({ onJev: true, keyStored: false, typeSafeOff: false })).toBe(true);
  });

  /** Same reason, one layer up: the selection is still Jev, so there has to be
   *  a way off it even with the provider parked. */
  it('is offered to a site already on Jev while TypeSafe is switched off', () => {
    expect(jevRowOffered({ onJev: true, keyStored: true, typeSafeOff: true })).toBe(true);
  });
});

describe('judgmentModelChoices', () => {
  const base: ModelChoice[] = [
    { value: 'claude-haiku-4-5', label: 'Haiku 4.5', reasoningEfforts: ['none', 'low'] },
  ];

  it('leaves the model list untouched when Jev is not offered', () => {
    expect(judgmentModelChoices(base, false)).toEqual(base);
  });

  /** Last, after the chat models, and with no tiers: picking it is one step,
   *  exactly as an image model is. */
  it('appends the Jev row with no reasoning tiers', () => {
    const rows = judgmentModelChoices(base, true);
    expect(rows).toHaveLength(2);
    expect(rows[1]).toMatchObject({
      value: JEV_MODEL_VALUE,
      label: JEV_MODEL_LABEL,
      reasoningEfforts: [],
    });
  });
});

describe('judgmentSelectedModel', () => {
  it('shows the Jev row while the site runs on Jev', () => {
    expect(judgmentSelectedModel(true, 'claude-haiku-4-5')).toBe(JEV_MODEL_VALUE);
    expect(judgmentSelectedEffort(true, 'none')).toBeNull();
  });

  it('shows the stored chat pair otherwise', () => {
    expect(judgmentSelectedModel(false, 'claude-haiku-4-5')).toBe('claude-haiku-4-5');
    expect(judgmentSelectedEffort(false, 'none')).toBe('none');
  });
});

describe('judgmentPickWrites', () => {
  /** Both halves, because the site may have been on Jev. Writing the model
   *  alone would leave the engine on Jev under a field showing Haiku. */
  it('moves the backend back to chat and writes the pair', () => {
    expect(judgmentPickWrites({ model: 'claude-haiku-4-5', reasoningEffort: 'low' })).toEqual({
      judgment: CHAT,
      selection: { model: 'claude-haiku-4-5', reasoningEffort: 'low' },
    });
  });

  /** The stored model is what switching back restores, so a Jev pick must not
   *  touch it. */
  it('writes only the judgment key when Jev is picked', () => {
    expect(judgmentPickWrites({ model: JEV_MODEL_VALUE, reasoningEffort: null })).toEqual({
      judgment: JEV,
      selection: null,
    });
  });

  it('writes only the two literals the engine reads', () => {
    const chat = judgmentPickWrites({ model: 'gemini-3.5-flash', reasoningEffort: null });
    expect(wantsJev(chat.judgment)).toBe(false);
    expect(wantsJev(judgmentPickWrites({ model: JEV_MODEL_VALUE, reasoningEffort: null }).judgment))
      .toBe(true);
  });
});

describe('judgmentSelectionCaveat', () => {
  /** `jev_for` returns nothing while the master switch is off, so a field
   *  reading Jev with nothing said would be claiming a backend nothing runs. */
  it('says so when the selection is Jev and TypeSafe is off', () => {
    expect(judgmentSelectionCaveat(true, true)).toContain('Models → Providers');
  });

  it('is silent in every other combination', () => {
    expect(judgmentSelectionCaveat(true, false)).toBeNull();
    expect(judgmentSelectionCaveat(false, true)).toBeNull();
    expect(judgmentSelectionCaveat(false, false)).toBeNull();
  });
});
