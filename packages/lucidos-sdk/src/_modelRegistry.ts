import { clampToOffered, type ModelChoice } from '../../../crates/lucidos-app/src/store/modelSelection';
import { providerLabel } from '../../../crates/lucidos-app/src/store/modelProviders';
import { REASONING_LEVELS } from '../../../crates/lucidos-app/src/store/reasoningLevels';

export type ModelProxyProtocol = 'anthropic-messages' | 'openai-responses'
  | 'openai-chat-completions' | 'vertex-anthropic' | 'vertex-google';

/** Safe projection served by GET /models. No credentials or private origins. */
export interface RegistryRoute {
  provider: string;
  id: string;
  reasoning_efforts: readonly string[];
  configured: boolean | null;
  proxy: {
    name: string;
    protocol: ModelProxyProtocol;
    request_defaults?: Record<string, { body: Record<string, unknown>; headers: Record<string, string> }>;
  } | null;
}

export interface RegistryModel {
  id: string;
  label: string;
  routes: readonly RegistryRoute[];
  preferred_provider: string | null;
  default_effort: string | null;
  successor: string | null;
  on_chat_ladder: boolean;
}

export interface ModelPickerValue {
  model: string;
  reasoning_effort: string | null;
  provider: string | null;
}

export interface ModelPickerFilter {
  callableOnly?: boolean;
  providers?: readonly string[];
  models?: readonly string[];
}

const protocols: readonly string[] = [
  'anthropic-messages', 'openai-responses', 'openai-chat-completions',
  'vertex-anthropic', 'vertex-google',
];

export function callableRoute(route: RegistryRoute): boolean {
  return !!route.proxy?.name && protocols.includes(route.proxy.protocol);
}

/** Filter first, then resolve defaults exclusively inside the offered routes. */
export function modelChoices(rows: readonly RegistryModel[], filter: ModelPickerFilter = {}): ModelChoice[] {
  const choices: ModelChoice[] = [];
  for (const row of rows) {
    if (row.successor || (filter.models && !filter.models.includes(row.id))) continue;
    const routes = row.routes.filter(route =>
      (!filter.callableOnly || callableRoute(route))
      && (!filter.providers || filter.providers.includes(route.provider)));
    const selected = routes.find(route => route.provider === row.preferred_provider)
      ?? routes.find(route => route.configured === true) ?? routes[0];
    if (!selected) continue;
    choices.push({
      value: row.id,
      label: row.label,
      section: row.on_chat_ladder ? 'Chat models' : 'Other models',
      defaultProvider: selected.provider,
      reasoningEfforts: selected.reasoning_efforts,
      providers: routes.map(route => ({
        value: route.provider,
        label: providerLabel(route.provider),
        configured: route.configured === true,
        reasoningEfforts: route.reasoning_efforts,
      })),
    });
  }
  return [
    ...choices.filter(row => row.section === 'Chat models'),
    ...choices.filter(row => row.section === 'Other models'),
  ];
}

/** Explicit unavailable selections are errors; only null requests the default. */
export function resolveSelection(choices: readonly ModelChoice[], value: ModelPickerValue): ModelPickerValue {
  const model = choices.find(row => row.value === value.model);
  if (!model) throw new Error(`Model is unavailable: ${value.model}`);
  const provider = value.provider ?? model.defaultProvider;
  const route = model.providers?.find(row => row.value === provider);
  if (!route) throw new Error(`Provider is unavailable for ${value.model}: ${provider}`);
  return {
    model: value.model,
    provider: route.value,
    reasoning_effort: clampToOffered(value.reasoning_effort,
      REASONING_LEVELS.filter(tier => route.reasoningEfforts.includes(tier.value))),
  };
}
