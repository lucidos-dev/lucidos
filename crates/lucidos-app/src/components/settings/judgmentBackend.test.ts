import { FLAG_OFF_VALUES, FLAG_ON_VALUES } from '@lucidos/preference-catalog';
import { WAKE_DECISION_DEFAULT_ENDPOINT } from '@lucidos/engine-constants';
import { describe, it, expect } from 'vitest';
import {
  CHAT,
  JUDGMENT_MODEL_KEY,
  JUDGMENT_PREFERENCE_KEY,
  JUDGMENT_REASONING_KEY,
  SYSTEM_ONE_ENDPOINTS,
  judgmentModelChoices,
  judgmentPickWrites,
  judgmentSelectedEffort,
  judgmentSelectedModel,
  judgmentSelectionCaveat,
  offeredEndpoints,
  pickedEndpoint,
  sitePick,
  systemOneConfigured,
  systemOneConsumers,
  systemOneSwitchedOff,
  workersAiAccount,
  workersAiScope,
  type SystemOneEndpointSpec,
  type SystemOneProviderId,
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

function loaded<T>(data: T): Loadable<T> {
  return { status: 'loaded', data };
}

const NO_PREFS = loaded<Record<string, string>>({});

function endpoint(id: SystemOneEndpointSpec['id']): SystemOneEndpointSpec {
  const found = SYSTEM_ONE_ENDPOINTS.find((e) => e.id === id);
  if (!found) throw new Error(`no endpoint ${id}`);
  return found;
}

describe('pickedEndpoint', () => {
  it('reads every row id, forgiving case and space', () => {
    expect(pickedEndpoint('jev')?.id).toBe('jev');
    expect(pickedEndpoint('  JEV  ')?.id).toBe('jev');
    expect(pickedEndpoint('clef')?.id).toBe('clef');
    expect(pickedEndpoint('Clef-Flash')?.id).toBe('clef-flash');
    expect(pickedEndpoint('openrouter')?.id).toBe('openrouter');
    expect(pickedEndpoint('OpenAI')?.id).toBe('openai');
    expect(pickedEndpoint('custom')?.id).toBe('custom');
  });

  it('reads every other value as chat', () => {
    for (const value of ['chat', '', '   ', 'jevvy', 'typesafe', undefined, null]) {
      expect(pickedEndpoint(value), String(value)).toBeNull();
    }
  });

  it('gives every row its own picker value', () => {
    const values = SYSTEM_ONE_ENDPOINTS.map((e) => e.choice.value);
    expect(new Set(values).size).toBe(values.length);
  });
});

describe('JUDGMENT_PREFERENCE_KEY', () => {
  it('names the four engine preference keys', () => {
    expect(JUDGMENT_PREFERENCE_KEY['command-guard']).toBe('judgment_command_guard');
    expect(JUDGMENT_PREFERENCE_KEY['query-classification'])
      .toBe('judgment_query_classification');
    expect(JUDGMENT_PREFERENCE_KEY['read-decision']).toBe('judgment_read_decision');
    expect(JUDGMENT_PREFERENCE_KEY['wake-decision']).toBe('judgment_wake_decision');
  });

  it('pairs each site with its own model and reasoning keys', () => {
    expect(JUDGMENT_MODEL_KEY['command-guard']).toBe('model_command_judge');
    expect(JUDGMENT_REASONING_KEY['command-guard']).toBe('reasoning_command_judge');
    expect(JUDGMENT_MODEL_KEY['query-classification']).toBe('model_query_classification');
    expect(JUDGMENT_REASONING_KEY['query-classification'])
      .toBe('reasoning_query_classification');
    expect(JUDGMENT_MODEL_KEY['read-decision']).toBe('model_read_decision');
    expect(JUDGMENT_REASONING_KEY['read-decision']).toBe('reasoning_read_decision');
    expect(JUDGMENT_MODEL_KEY['wake-decision']).toBe('model_wake_decision');
    expect(JUDGMENT_REASONING_KEY['wake-decision']).toBe('reasoning_wake_decision');
  });
});

describe('systemOneConfigured', () => {
  it('sees a stored key for TypeSafe and Cloudflare', () => {
    expect(systemOneConfigured('typesafe', loaded([cred('typesafe')]), NO_PREFS, null)).toBe(true);
    expect(systemOneConfigured('cloudflare-workers-ai', loaded([cred('cloudflare-workers-ai')]), NO_PREFS, null))
      .toBe(true);
    expect(systemOneConfigured('cloudflare-workers-ai', loaded([cred('typesafe')]), NO_PREFS, null))
      .toBe(false);
  });

  it('is false while credentials load, or for an oauth_client row of the same name', () => {
    expect(systemOneConfigured('typesafe', { status: 'loading' }, NO_PREFS, null)).toBe(false);
    expect(systemOneConfigured('typesafe', loaded([cred('typesafe', 'oauth_client')]), NO_PREFS, null))
      .toBe(false);
  });

  it('reads the custom endpoint as set up by its URL and model, no key needed', () => {
    const url = 'http://localhost:8000/v1/systemone';
    const both = loaded({ system_one_custom_url: url, system_one_custom_model: 'kev' });
    expect(systemOneConfigured('system-one-custom', loaded([]), both, null)).toBe(true);
    expect(systemOneConfigured('system-one-custom', loaded([]), loaded({ system_one_custom_url: url }), null))
      .toBe(false);
    expect(systemOneConfigured('system-one-custom', loaded([cred('system-one-custom')]), NO_PREFS, null))
      .toBe(false);
  });
});

describe('systemOneConfigured for a chat provider row', () => {
  it('reads OpenRouter and OpenAI as set up once /health lists their chat provider', () => {
    expect(systemOneConfigured('openai', loaded([]), NO_PREFS, ['openai'])).toBe(true);
    expect(systemOneConfigured('openrouter', loaded([]), NO_PREFS, ['openrouter'])).toBe(true);
    expect(systemOneConfigured('openai', loaded([]), NO_PREFS, ['openrouter'])).toBe(false);
    expect(systemOneConfigured('openai', loaded([]), NO_PREFS, null)).toBe(false);
  });

  it('still reads a stored key before /health answers', () => {
    expect(systemOneConfigured('openrouter', loaded([cred('openrouter')]), NO_PREFS, null)).toBe(true);
  });

  it('never reads TypeSafe as set up from a chat provider', () => {
    expect(systemOneConfigured('typesafe', loaded([]), NO_PREFS, ['openai', 'openrouter'])).toBe(false);
  });
});

describe('systemOneSwitchedOff', () => {
  const stored = (provider: SystemOneProviderId, key: string, value?: string) =>
    systemOneSwitchedOff(loaded(value === undefined ? {} : { [key]: value }), provider);

  it('is false while unset, set to true, or still loading', () => {
    expect(stored('typesafe', 'provider_enabled_typesafe')).toBe(false);
    expect(stored('typesafe', 'provider_enabled_typesafe', 'true')).toBe(false);
    expect(systemOneSwitchedOff({ status: 'loading' }, 'typesafe')).toBe(false);
  });

  it('reads every spelling the engine reads as off, on each provider’s own key', () => {
    for (const spelled of FLAG_OFF_VALUES) {
      for (const value of [spelled, spelled.toUpperCase(), `  ${spelled}  `]) {
        expect(stored('typesafe', 'provider_enabled_typesafe', value), value).toBe(true);
      }
    }
    for (const value of FLAG_ON_VALUES) {
      expect(stored('typesafe', 'provider_enabled_typesafe', value), value).toBe(false);
    }
    expect(stored('cloudflare-workers-ai', 'provider_enabled_cloudflare_workers_ai', 'false')).toBe(true);
    expect(stored('system-one-custom', 'provider_enabled_system_one_custom', 'false')).toBe(true);
    expect(stored('cloudflare-workers-ai', 'provider_enabled_typesafe', 'false')).toBe(false);
    expect(stored('openrouter', 'provider_enabled_openrouter', 'false')).toBe(true);
    expect(stored('openai', 'provider_enabled_openai', 'false')).toBe(true);
  });
});

describe('sitePick', () => {
  describe.each(['wake-decision'] as const)('%s', (site) => {
    const backend = JUDGMENT_PREFERENCE_KEY[site];
    const model = JUDGMENT_MODEL_KEY[site];

    it('shows OpenAI Decisions while OpenAI is served and unset', () => {
      expect(sitePick(site, NO_PREFS, ['openai'])?.id).toBe(WAKE_DECISION_DEFAULT_ENDPOINT);
      expect(sitePick(site, loaded({ [backend]: '  ' }), ['openai'])?.id)
        .toBe(WAKE_DECISION_DEFAULT_ENDPOINT);
    });

    it('shows chat when OpenAI is not served, switched off, or chat was picked', () => {
      expect(sitePick(site, NO_PREFS, ['vertex'])).toBeNull();
      expect(sitePick(site, NO_PREFS, null)).toBeNull();
      expect(sitePick(site, loaded({ provider_enabled_openai: 'false' }), ['openai'])).toBeNull();
      expect(sitePick(site, loaded({ [backend]: 'chat' }), ['openai'])).toBeNull();
    });

    it('keeps a pinned chat model on chat', () => {
      expect(sitePick(site, loaded({ [model]: 'local-model' }), ['openai'])).toBeNull();
    });

    it('lets a stored pick decide', () => {
      expect(sitePick(site, loaded({ [backend]: 'jev' }), ['openai'])?.id).toBe('jev');
    });
  });

  it('gives no other site a default, the read decision included', () => {
    expect(sitePick('read-decision', NO_PREFS, ['openai', 'openrouter'])).toBeNull();
    expect(sitePick('command-guard', NO_PREFS, ['openai', 'openrouter'])).toBeNull();
    expect(sitePick('query-classification', NO_PREFS, ['openai', 'openrouter'])).toBeNull();
  });
});

describe('systemOneConsumers', () => {
  it('lists the judge tool for TypeSafe on a key alone, ahead of any site', () => {
    expect(systemOneConsumers('typesafe', NO_PREFS, true, null)).toEqual(['The agent’s judge tool']);
    expect(systemOneConsumers('typesafe', NO_PREFS, false, null)).toEqual([]);
  });

  it('never lists the judge tool for another provider', () => {
    expect(systemOneConsumers('cloudflare-workers-ai', NO_PREFS, true, null)).toEqual([]);
    expect(systemOneConsumers('openai', NO_PREFS, true, null)).toEqual([]);
  });

  it('names the wake decision on OpenAI by default while OpenAI is served', () => {
    expect(systemOneConsumers('openai', NO_PREFS, true, ['openai']))
      .toEqual(['Wake decision']);
    expect(systemOneConsumers('openai', NO_PREFS, true, null)).toEqual([]);
  });

  it('names the read decision on the provider it picked', () => {
    const prefs = loaded({ judgment_read_decision: 'openai' });
    expect(systemOneConsumers('openai', prefs, true, null)).toEqual(['Read decision']);
    expect(systemOneConsumers('typesafe', prefs, false, null)).toEqual([]);
  });

  it('names each site picked on that provider, and only those', () => {
    const prefs = loaded({ judgment_command_guard: 'clef-flash', judgment_query_classification: 'jev' });
    expect(systemOneConsumers('cloudflare-workers-ai', prefs, true, null)).toEqual(['Command guard']);
    expect(systemOneConsumers('typesafe', prefs, true, null))
      .toEqual(['The agent’s judge tool', 'Query classification']);
  });
});

describe('offeredEndpoints', () => {
  const ids = (args: Parameters<typeof offeredEndpoints>[0]) =>
    offeredEndpoints(args).map((e) => e.id);

  it('offers nothing with no provider set up', () => {
    expect(ids({ picked: null, configured: () => false, switchedOff: () => false })).toEqual([]);
  });

  it('offers every row of each provider that is set up and on', () => {
    expect(ids({
      picked: null,
      configured: (p) => p === 'cloudflare-workers-ai',
      switchedOff: () => false,
    })).toEqual(['clef', 'clef-flash']);
  });

  it('offers no row of a switched-off provider', () => {
    expect(ids({ picked: null, configured: () => true, switchedOff: (p) => p === 'typesafe' }))
      .toEqual(['openrouter', 'clef', 'clef-flash', 'openai', 'custom']);
  });

  it('always offers the row the site is on', () => {
    expect(ids({ picked: endpoint('jev'), configured: () => false, switchedOff: () => true }))
      .toEqual(['jev']);
  });
});

describe('judgmentModelChoices', () => {
  const base: ModelChoice[] = [
    { value: 'claude-haiku-4-5-20251001', label: 'Haiku 4.5', reasoningEfforts: ['none', 'low'] },
  ];

  it('leaves the model list untouched when nothing is offered', () => {
    expect(judgmentModelChoices(base, [])).toEqual(base);
  });

  it('appends each offered row with no reasoning tiers', () => {
    const rows = judgmentModelChoices(base, [endpoint('jev'), endpoint('clef')]);
    expect(rows).toHaveLength(3);
    expect(rows[1]).toMatchObject({ label: 'TypeSafe (Jev)', reasoningEfforts: [] });
    expect(rows[2]).toMatchObject({ label: 'Cloudflare Clef', reasoningEfforts: [] });
  });
});

describe('judgmentSelectedModel', () => {
  it('shows the picked row while the site runs on it', () => {
    expect(judgmentSelectedModel(endpoint('clef'), 'claude-haiku-4-5-20251001'))
      .toBe(endpoint('clef').choice.value);
    expect(judgmentSelectedEffort(endpoint('clef'), 'none')).toBeNull();
  });

  it('shows the stored chat pair otherwise', () => {
    expect(judgmentSelectedModel(null, 'claude-haiku-4-5-20251001')).toBe('claude-haiku-4-5-20251001');
    expect(judgmentSelectedEffort(null, 'none')).toBe('none');
  });
});

describe('judgmentPickWrites', () => {
  it('moves the backend back to chat and writes the pair', () => {
    expect(judgmentPickWrites({ model: 'claude-haiku-4-5-20251001', reasoningEffort: 'low', provider: null })).toEqual({
      judgment: CHAT,
      selection: { model: 'claude-haiku-4-5-20251001', reasoningEffort: 'low', provider: null },
    });
  });

  it('writes only the row id for a System One pick', () => {
    for (const e of SYSTEM_ONE_ENDPOINTS) {
      const writes = judgmentPickWrites({ model: e.choice.value, reasoningEffort: null, provider: null });
      expect(writes).toEqual({ judgment: e.id, selection: null });
      expect(pickedEndpoint(writes.judgment)?.id).toBe(e.id);
    }
  });
});

describe('judgmentSelectionCaveat', () => {
  it('names the switched-off provider of the picked row', () => {
    const caveat = judgmentSelectionCaveat(endpoint('clef-flash'), true);
    expect(caveat).toContain('Cloudflare Workers AI');
    expect(caveat).toContain('Settings → Providers');
  });

  it('is silent in every other combination', () => {
    expect(judgmentSelectionCaveat(endpoint('jev'), false)).toBeNull();
    expect(judgmentSelectionCaveat(null, true)).toBeNull();
    expect(judgmentSelectionCaveat(null, false)).toBeNull();
  });
});

describe('workersAiScope', () => {
  it('builds the scope the engine reads the account from', () => {
    expect(workersAiScope(' abc123 '))
      .toBe('https://api.cloudflare.com/client/v4/accounts/abc123/ai');
    expect(workersAiAccount(workersAiScope('abc123') ?? undefined)).toBe('abc123');
  });

  it('refuses an id the engine would refuse', () => {
    for (const id of ['', 'abc/../x', 'abc 123', 'abc-123']) {
      expect(workersAiScope(id), id).toBeNull();
    }
    expect(workersAiAccount('https://evil.example/client/v4/accounts/abc/ai')).toBeNull();
    expect(workersAiAccount(undefined)).toBeNull();
  });
});
