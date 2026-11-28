import type { CredentialInfo, Loadable } from '../../store/types';
import {
  PROVIDER_ENABLED_CLOUDFLARE_WORKERS_AI_KEY,
  PROVIDER_ENABLED_OPENAI_KEY,
  PROVIDER_ENABLED_OPENROUTER_KEY,
  PROVIDER_ENABLED_SYSTEM_ONE_CUSTOM_KEY,
  PROVIDER_ENABLED_TYPESAFE_KEY,
  SYSTEM_ONE_CUSTOM_MODEL_KEY,
  SYSTEM_ONE_CUSTOM_URL_KEY,
  switchValueReadsAsOff,
  type BackgroundModelKey,
  type BackgroundReasoningKey,
  type SwitchableProvider,
} from '../../store/actions/preferences';
import type { ModelSelectionPatch } from '../../hooks/useModelSelection';
import type { ModelChoice } from '../../store/modelSelection';
import { PROVIDERS_SETTINGS_PATH, providerAnchor } from '../../store/modelProviders';
import { findProviderCredential } from './providerCredential';
import {
  CLOUDFLARE_ACCOUNTS_PREFIX,
  CLOUDFLARE_WORKERS_AI_CREDENTIAL_SERVICE,
  SYSTEM_ONE_CUSTOM_CREDENTIAL_SERVICE,
  SYSTEM_ONE_ENDPOINT_IDS,
  TYPESAFE_CREDENTIAL_SERVICE,
  WAKE_DECISION_DEFAULT_ENDPOINT,
} from '@lucidos/engine-constants';
import {
  PREF_JUDGMENT_COMMAND_GUARD,
  PREF_JUDGMENT_QUERY_CLASSIFICATION,
  PREF_JUDGMENT_READ_DECISION,
  PREF_JUDGMENT_WAKE_DECISION,
  PREF_JUDGMENT_EXPECTED_WAKE_DECISION,
  PREF_MODEL_EXPECTED_WAKE_DECISION,
  PREF_REASONING_EXPECTED_WAKE_DECISION,
  PREF_JUDGMENT_PERMISSION_ANSWER,
  PREF_MODEL_PERMISSION_ANSWER,
  PREF_REASONING_PERMISSION_ANSWER,
  PREF_MODEL_WAKE_DECISION,
  PREF_REASONING_WAKE_DECISION,
} from '@lucidos/preference-catalog';


export const CHAT = 'chat';

export { TYPESAFE_API_KEY_ENV } from '@lucidos/engine-constants';

export type SystemOneProviderId =
  | 'typesafe' | 'openrouter' | 'cloudflare-workers-ai' | 'openai' | 'system-one-custom';

export interface SystemOneProviderSpec {
  id: SystemOneProviderId;
  label: string;
  anchor: string;
  credentialService: string;
  switchKey: string;
  chatProvider?: SwitchableProvider;
}

function chatProviderRow(
  id: SystemOneProviderId & SwitchableProvider,
  label: string,
  switchKey: string,
): SystemOneProviderSpec {
  return { id, label, anchor: providerAnchor(id), credentialService: id, switchKey, chatProvider: id };
}

export const SYSTEM_ONE_PROVIDERS: Record<SystemOneProviderId, SystemOneProviderSpec> = {
  typesafe: {
    id: 'typesafe',
    label: 'TypeSafe (Jev)',
    anchor: providerAnchor('typesafe'),
    credentialService: TYPESAFE_CREDENTIAL_SERVICE,
    switchKey: PROVIDER_ENABLED_TYPESAFE_KEY,
  },
  openrouter: chatProviderRow('openrouter', 'OpenRouter', PROVIDER_ENABLED_OPENROUTER_KEY),
  openai: chatProviderRow('openai', 'OpenAI', PROVIDER_ENABLED_OPENAI_KEY),
  'cloudflare-workers-ai': {
    id: 'cloudflare-workers-ai',
    label: 'Cloudflare Workers AI (Clef)',
    anchor: providerAnchor('cloudflare-workers-ai'),
    credentialService: CLOUDFLARE_WORKERS_AI_CREDENTIAL_SERVICE,
    switchKey: PROVIDER_ENABLED_CLOUDFLARE_WORKERS_AI_KEY,
  },
  'system-one-custom': {
    id: 'system-one-custom',
    label: 'Custom System One endpoint',
    anchor: providerAnchor('system-one-custom'),
    credentialService: SYSTEM_ONE_CUSTOM_CREDENTIAL_SERVICE,
    switchKey: PROVIDER_ENABLED_SYSTEM_ONE_CUSTOM_KEY,
  },
};

export interface SystemOneEndpointSpec {
  id: (typeof SYSTEM_ONE_ENDPOINT_IDS)[number];
  provider: SystemOneProviderId;
  choice: ModelChoice;
}

const TYPED = 'Typed judgment rather than a written answer';

export const SYSTEM_ONE_ENDPOINTS: readonly SystemOneEndpointSpec[] = [
  {
    id: 'jev',
    provider: 'typesafe',
    choice: { value: 'typesafe-jev', label: 'TypeSafe (Jev)', description: TYPED, reasoningEfforts: [] },
  },
  {
    id: 'openrouter',
    provider: 'openrouter',
    choice: {
      value: 'openrouter-system-one',
      label: 'OpenRouter System One',
      description: 'Jev, or the System One model named on the OpenRouter row',
      reasoningEfforts: [],
    },
  },
  {
    id: 'clef',
    provider: 'cloudflare-workers-ai',
    choice: { value: 'cloudflare-clef', label: 'Cloudflare Clef', description: TYPED, reasoningEfforts: [] },
  },
  {
    id: 'clef-flash',
    provider: 'cloudflare-workers-ai',
    choice: {
      value: 'cloudflare-clef-flash',
      label: 'Cloudflare Clef-flash',
      description: 'Fast typed judgment',
      reasoningEfforts: [],
    },
  },
  {
    id: 'openai',
    provider: 'openai',
    choice: {
      value: 'openai-decisions',
      label: 'OpenAI Decisions',
      description: 'GPT-6 Luna, typed judgment',
      reasoningEfforts: [],
    },
  },
  {
    id: 'custom',
    provider: 'system-one-custom',
    choice: { value: 'system-one-custom', label: 'Custom System One', description: TYPED, reasoningEfforts: [] },
  },
];

export type JudgmentSite =
  | 'command-guard'
  | 'query-classification'
  | 'read-decision'
  | 'wake-decision'
  | 'expected-wake-decision'
  | 'permission-answer';

export const JUDGMENT_SITES: readonly JudgmentSite[] = [
  'command-guard',
  'query-classification',
  'read-decision',
  'wake-decision',
  'expected-wake-decision',
  'permission-answer',
];

export const JUDGMENT_PREFERENCE_KEY: Record<JudgmentSite, string> = {
  'command-guard': PREF_JUDGMENT_COMMAND_GUARD.key,
  'query-classification': PREF_JUDGMENT_QUERY_CLASSIFICATION.key,
  'read-decision': PREF_JUDGMENT_READ_DECISION.key,
  'wake-decision': PREF_JUDGMENT_WAKE_DECISION.key,
  'expected-wake-decision': PREF_JUDGMENT_EXPECTED_WAKE_DECISION.key,
  'permission-answer': PREF_JUDGMENT_PERMISSION_ANSWER.key,
};

export const JUDGMENT_MODEL_KEY: Record<JudgmentSite, BackgroundModelKey> = {
  'command-guard': 'model_command_judge',
  'query-classification': 'model_query_classification',
  'read-decision': 'model_read_decision',
  'wake-decision': PREF_MODEL_WAKE_DECISION.key,
  'expected-wake-decision': PREF_MODEL_EXPECTED_WAKE_DECISION.key,
  'permission-answer': PREF_MODEL_PERMISSION_ANSWER.key,
};

export const JUDGMENT_REASONING_KEY: Record<JudgmentSite, BackgroundReasoningKey> = {
  'command-guard': 'reasoning_command_judge',
  'query-classification': 'reasoning_query_classification',
  'read-decision': 'reasoning_read_decision',
  'wake-decision': PREF_REASONING_WAKE_DECISION.key,
  'expected-wake-decision': PREF_REASONING_EXPECTED_WAKE_DECISION.key,
  'permission-answer': PREF_REASONING_PERMISSION_ANSWER.key,
};

export const JUDGMENT_SITE_LABEL: Record<JudgmentSite, string> = {
  'command-guard': 'Command guard',
  'query-classification': 'Query classification',
  'read-decision': 'Read decision',
  'wake-decision': 'Wake decision',
  'expected-wake-decision': 'Expected wake',
  'permission-answer': 'Permission answer',
};

export const JUDGMENT_SITE_LOCATION: Record<JudgmentSite, string> = {
  'command-guard': 'Permissions → Command safety',
  'query-classification': 'Models → Background tasks',
  'read-decision': 'Models → Background tasks',
  'wake-decision': 'Models → Background tasks',
  'expected-wake-decision': 'Models → Background tasks',
  'permission-answer': 'Models → Background tasks',
};

export const JUDGMENT_SITE_DEFAULT: Partial<Record<JudgmentSite, string>> = {
  'wake-decision': WAKE_DECISION_DEFAULT_ENDPOINT,
};

export function sitePick(
  site: JudgmentSite,
  prefs: Loadable<Record<string, string>>,
  chatProviders: readonly string[] | null,
): SystemOneEndpointSpec | null {
  const stored = storedJudgmentValue(prefs, site);
  if (stored?.trim()) return pickedEndpoint(stored);
  if (prefs.status !== 'loaded' || prefs.data[JUDGMENT_MODEL_KEY[site]]?.trim()) return null;
  const fallback = pickedEndpoint(JUDGMENT_SITE_DEFAULT[site]);
  const chatProvider = fallback ? SYSTEM_ONE_PROVIDERS[fallback.provider].chatProvider : undefined;
  const served = !!chatProvider && !!chatProviders?.includes(chatProvider);
  return fallback && served && !systemOneSwitchedOff(prefs, fallback.provider) ? fallback : null;
}

export function pickedEndpoint(value: string | undefined | null): SystemOneEndpointSpec | null {
  const id = value?.trim().toLowerCase();
  return SYSTEM_ONE_ENDPOINTS.find((e) => e.id === id) ?? null;
}

export function systemOneConfigured(
  provider: SystemOneProviderId,
  credentials: Loadable<CredentialInfo[]>,
  prefs: Loadable<Record<string, string>>,
  chatProviders: readonly string[] | null,
): boolean {
  const chatProvider = SYSTEM_ONE_PROVIDERS[provider].chatProvider;
  if (chatProvider && chatProviders?.includes(chatProvider)) return true;
  if (provider === 'system-one-custom') {
    return prefs.status === 'loaded'
      && !!prefs.data[SYSTEM_ONE_CUSTOM_URL_KEY]?.trim()
      && !!prefs.data[SYSTEM_ONE_CUSTOM_MODEL_KEY]?.trim();
  }
  return !!findProviderCredential(credentials, SYSTEM_ONE_PROVIDERS[provider].credentialService);
}

export function systemOneSwitchedOff(
  prefs: Loadable<Record<string, string>>,
  provider: SystemOneProviderId,
): boolean {
  return prefs.status === 'loaded'
    && switchValueReadsAsOff(prefs.data[SYSTEM_ONE_PROVIDERS[provider].switchKey]);
}

export function storedJudgmentValue(
  prefs: Loadable<Record<string, string>>,
  site: JudgmentSite,
): string | undefined {
  return prefs.status === 'loaded' ? prefs.data[JUDGMENT_PREFERENCE_KEY[site]] : undefined;
}

export const JUDGE_TOOL_LABEL = 'The agent’s judge tool';

export function systemOneConsumers(
  provider: SystemOneProviderId,
  prefs: Loadable<Record<string, string>>,
  keyStored: boolean,
  chatProviders: readonly string[] | null,
): string[] {
  const sites = JUDGMENT_SITES
    .filter((site) => sitePick(site, prefs, chatProviders)?.provider === provider)
    .map((site) => JUDGMENT_SITE_LABEL[site]);
  return provider === 'typesafe' && keyStored ? [JUDGE_TOOL_LABEL, ...sites] : sites;
}

export function offeredEndpoints(args: {
  picked: SystemOneEndpointSpec | null;
  configured: (provider: SystemOneProviderId) => boolean;
  switchedOff: (provider: SystemOneProviderId) => boolean;
}): SystemOneEndpointSpec[] {
  return SYSTEM_ONE_ENDPOINTS.filter((endpoint) =>
    endpoint.id === args.picked?.id
    || (args.configured(endpoint.provider) && !args.switchedOff(endpoint.provider)));
}

export const SYSTEM_ONE_SECTION = 'System One';

export function judgmentModelChoices(
  base: readonly ModelChoice[],
  offered: readonly SystemOneEndpointSpec[],
): ModelChoice[] {
  const sectioned = base.some((choice) => choice.section);
  return [
    ...base,
    ...offered.map((endpoint) => (
      sectioned ? { ...endpoint.choice, section: SYSTEM_ONE_SECTION } : endpoint.choice
    )),
  ];
}

export function judgmentSelectedModel(
  picked: SystemOneEndpointSpec | null,
  storedModel: string,
): string {
  return picked ? picked.choice.value : storedModel;
}

export function judgmentSelectedEffort(
  picked: SystemOneEndpointSpec | null,
  storedEffort: string | null,
): string | null {
  return picked ? null : storedEffort;
}

export interface JudgmentPickWrites {
  judgment: string;
  selection: ModelSelectionPatch | null;
}

export function judgmentPickWrites(patch: ModelSelectionPatch): JudgmentPickWrites {
  const endpoint = SYSTEM_ONE_ENDPOINTS.find((e) => e.choice.value === patch.model);
  if (endpoint) return { judgment: endpoint.id, selection: null };
  return { judgment: CHAT, selection: patch };
}

export function judgmentSelectionCaveat(
  picked: SystemOneEndpointSpec | null,
  switchedOff: boolean,
): string | null {
  if (!picked || !switchedOff) return null;
  return `${SYSTEM_ONE_PROVIDERS[picked.provider].label} is switched off in `
    + `${PROVIDERS_SETTINGS_PATH}, so this runs on its chat model`;
}

export function workersAiScope(accountId: string): string | null {
  const id = accountId.trim();
  return /^[A-Za-z0-9]+$/.test(id) ? `${CLOUDFLARE_ACCOUNTS_PREFIX}${id}/ai` : null;
}

export function workersAiAccount(scope: string | undefined): string | null {
  const match = scope?.trim().replace(/\/+$/, '').match(/^https:\/\/api\.cloudflare\.com\/client\/v4\/accounts\/([A-Za-z0-9]+)\/ai$/);
  return match ? match[1] : null;
}
