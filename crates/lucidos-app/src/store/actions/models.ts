import {
  chatLadder, chatModels, configuredProviders, showToast, showConfirm,
} from '../store';
import { failedIfFresh, toFailed, setLoadingIfFresh } from '../types';
import { loadSlice, type SliceOf } from '../bootstrapSliceLoads';
import { MODELS, PROVIDERS, REASONING_LEVELS, providerLabel } from '../models';
import {
  clampToOffered, lucidosTiers, tierOptions,
  type ModelChoice, type ProviderChoice, type TierChoice,
} from '../modelSelection';
import { displayModelName } from '../thread-events';
import {
  getChatLadder, listModels, createModel, updateModel, deleteModelApi, type RouteInput,
} from '../../api/client';
import type { ModelInfo, RouteInfo } from '../../api/types';
import { errorDetail } from '../../utils/errorDetail';

export function isProviderConfigured(provider: string): boolean {
  const set = configuredProviders.value;
  return set === null || set.includes(provider);
}

export const chatModelsSlice: SliceOf<'chat_models'> = {
  begin: () => setLoadingIfFresh(chatModels),
  apply: (data) => { chatModels.value = { status: 'loaded', data: data.models || [] }; },
  fail: (error) => { chatModels.value = toFailed(error); },
};

export function loadChatModels(): Promise<void> {
  return loadSlice('chat_models', chatModelsSlice, () => listModels());
}

let chatLadderReadSeq = 0;

export const chatLadderSlice: SliceOf<'intelligence_ladder', number> = {
  begin: () => {
    const seq = ++chatLadderReadSeq;
    setLoadingIfFresh(chatLadder);
    return seq;
  },
  apply: (data, seq) => {
    if (seq === chatLadderReadSeq) chatLadder.value = { status: 'loaded', data };
  },
  fail: (error, seq) => {
    if (seq === chatLadderReadSeq) chatLadder.value = failedIfFresh(chatLadder.peek(), error);
  },
};

export function loadChatLadder(): Promise<void> {
  return loadSlice('intelligence_ladder', chatLadderSlice, () => getChatLadder());
}

export function modelRow(modelId: string): ModelInfo | undefined {
  const loadable = chatModels.value;
  if (loadable.status !== 'loaded') return undefined;
  return loadable.data.find((m) => m.id === modelId);
}

export function hasConfiguredRoute(model: ModelInfo): boolean {
  return model.routes.some((r) => isProviderConfigured(r.provider));
}

export interface RegistryGroups {
  current: ModelInfo[];
  needsProvider: ModelInfo[];
  retired: ModelInfo[];
}

export function groupRegistryModels(
  models: readonly ModelInfo[],
  isConfigured: (provider: string) => boolean,
): RegistryGroups {
  const groups: RegistryGroups = { current: [], needsProvider: [], retired: [] };
  for (const m of models) {
    if (m.successor !== null) groups.retired.push(m);
    else if (m.routes.some((r) => isConfigured(r.provider))) groups.current.push(m);
    else groups.needsProvider.push(m);
  }
  return groups;
}

export function providerSetupCount(configured: readonly string[] | null): number | null {
  if (configured === null) return null;
  return PROVIDERS.filter((p) => configured.includes(p.value)).length;
}

export function addModelProviderOptions(
  configured: readonly string[] | null,
): typeof PROVIDERS {
  return configured === null ? PROVIDERS : PROVIDERS.filter((p) => configured.includes(p.value));
}

export function readsImages(modelId: string): boolean {
  return modelRow(modelId)?.vision ?? false;
}

export function resolvedRoute(modelId: string): RouteInfo | undefined {
  const row = modelRow(modelId);
  if (!row) return undefined;
  const preferred = row.preferred_provider
    ? row.routes.find((r) => r.provider === row.preferred_provider)
    : undefined;
  return preferred
    ?? row.routes.find((r) => isProviderConfigured(r.provider))
    ?? row.routes.find((r) => r.provider !== 'chatgpt-plan')
    ?? row.routes[0];
}

export function modelReasoningEfforts(modelId: string): string[] | undefined {
  return resolvedRoute(modelId)?.reasoning_efforts;
}

export const LUCIDOS_TIER_VOCABULARY: readonly TierChoice[] = REASONING_LEVELS;

export const PROVIDER_DEFAULT_EFFORT_LABEL = 'Provider default';

export function modelDefaultEffort(modelId: string): string | null {
  const effort = modelRow(modelId)?.default_effort ?? null;
  return effort === null ? null : clampEffortFor(effort, modelId);
}

export function defaultTierLabel(effort: string | null): string {
  if (effort === null) return PROVIDER_DEFAULT_EFFORT_LABEL;
  const tier = LUCIDOS_TIER_VOCABULARY.find((t) => t.value === effort)?.label ?? effort;
  return `Default (${tier})`;
}

export function clampEffortFor(effort: string, modelId: string): string {
  const offered = tierOptions(
    lucidosTiers(modelReasoningEfforts(modelId)),
    LUCIDOS_TIER_VOCABULARY,
  );
  return clampToOffered(effort, offered) ?? effort;
}

function providerChoices(modelId: string): ProviderChoice[] | undefined {
  return modelRow(modelId)?.routes.map((r) => ({
    value: r.provider,
    label: providerLabel(r.provider),
    configured: isProviderConfigured(r.provider),
    reasoningEfforts: lucidosTiers(r.reasoning_efforts),
  }));
}

function lucidosModelChoice(value: string, label: string): ModelChoice {
  return {
    value,
    label,
    reasoningEfforts: lucidosTiers(modelReasoningEfforts(value)),
    providers: providerChoices(value),
    defaultProvider: resolvedRoute(value)?.provider,
  };
}

export function lucidosModelChoices(current?: string | null): ModelChoice[] {
  const choices = chatModelOptions().map((o) => lucidosModelChoice(o.value, o.label));
  if (current && !choices.some((c) => c.value === current)) {
    choices.push(lucidosModelChoice(current, displayModelName(current)));
  }
  return choices;
}

export function chatModelOptions(): Array<{ value: string; label: string }> {
  const loadable = chatModels.value;
  if (loadable.status === 'loaded') {
    return loadable.data
      .filter((m) => m.on_chat_ladder && hasConfiguredRoute(m))
      .map((m) => ({ value: m.id, label: m.label }));
  }
  return MODELS;
}

async function runModelMutation(
  apiCall: () => Promise<{ success: boolean; error?: string }>,
  failMsg: string
): Promise<boolean> {
  try {
    const data = await apiCall();
    if (!data.success) {
      showToast(data.error || failMsg, 'error');
      return false;
    }
    await loadChatModels();
    return true;
  } catch (error) {
    showToast(`${failMsg}: ${errorDetail(error)}`, 'error');
    return false;
  }
}

export function parseContextWindow(
  raw: string
): { ok: true; value: number | undefined } | { ok: false; error: string } {
  const trimmed = raw.trim();
  if (!trimmed) return { ok: true, value: undefined };
  const n = Number(trimmed);
  if (!Number.isInteger(n) || n <= 0) {
    return { ok: false, error: 'Context window must be a positive whole number of tokens' };
  }
  return { ok: true, value: n };
}

export async function submitNewModel(
  id: string,
  label: string,
  provider: string,
  contextWindow: string,
  vision: boolean
): Promise<boolean> {
  if (!id.trim() || !label.trim()) {
    showToast('Model id and label are required', 'error');
    return false;
  }
  const parsed = parseContextWindow(contextWindow);
  if (!parsed.ok) {
    showToast(parsed.error, 'error');
    return false;
  }
  return runModelMutation(
    () =>
      createModel({
        id: id.trim(),
        label: label.trim(),
        provider,
        context_window: parsed.value,
        vision,
      }),
    'Failed to add model'
  );
}

export async function rememberProvider(id: string, provider: string): Promise<void> {
  if (modelRow(id)?.preferred_provider === provider) return;
  await runModelMutation(
    () => updateModel(id, { preferred_provider: provider }),
    'Failed to remember the provider'
  );
}

export interface RouteDraft {
  provider: string;
  id: string;
  contextWindow: string;
}

export function routeDrafts(model: ModelInfo): RouteDraft[] {
  return model.routes.map((r) => ({
    provider: r.provider,
    id: r.id === model.id ? '' : r.id,
    contextWindow: r.context_window ? String(r.context_window) : '',
  }));
}

export function buildRoutes(
  modelId: string,
  drafts: readonly RouteDraft[],
): { ok: true; routes: RouteInput[] } | { ok: false; error: string } {
  if (drafts.length === 0) return { ok: false, error: 'A model needs at least one route' };
  const routes: RouteInput[] = [];
  for (const draft of drafts) {
    if (routes.some((r) => r.provider === draft.provider)) {
      return { ok: false, error: `${providerLabel(draft.provider)} appears twice` };
    }
    const window = parseContextWindow(draft.contextWindow);
    if (!window.ok) return { ok: false, error: `${providerLabel(draft.provider)}: ${window.error}` };
    const id = draft.id.trim();
    routes.push({
      provider: draft.provider,
      ...(id && id !== modelId ? { id } : {}),
      ...(window.value !== undefined ? { context_window: window.value } : {}),
    });
  }
  return { ok: true, routes };
}

export async function saveModelRoutes(
  modelId: string,
  drafts: readonly RouteDraft[],
  preferred: string | null | undefined,
): Promise<boolean> {
  const built = buildRoutes(modelId, drafts);
  if (!built.ok) {
    showToast(built.error, 'error');
    return false;
  }
  const pick = preferred && !built.routes.some((r) => r.provider === preferred) ? null : preferred;
  return runModelMutation(
    () => updateModel(modelId, {
      routes: built.routes,
      ...(pick !== undefined ? { preferred_provider: pick } : {}),
    }),
    'Failed to save the routes'
  );
}

export function setModelVision(id: string, vision: boolean): Promise<boolean> {
  return runModelMutation(() => updateModel(id, { vision }), 'Failed to update model');
}

export async function deleteModel(id: string): Promise<void> {
  if (!(await showConfirm(`Delete model "${id}"?`, 'Delete', { variant: 'danger' }))) {
    return;
  }
  await runModelMutation(() => deleteModelApi(id), 'Failed to delete model');
}
