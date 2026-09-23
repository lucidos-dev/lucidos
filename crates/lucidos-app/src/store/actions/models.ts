import {
  chatModels, configuredProviders, currentModel, reasoningEffort, showToast, showConfirm,
} from '../store';
import { toFailed, setLoadingIfFresh } from '../types';
import { MODELS, REASONING_LEVELS, providerLabel } from '../models';
import {
  clampToOffered, lucidosTiers, tierOptions,
  type ModelChoice, type ProviderChoice, type TierChoice,
} from '../modelSelection';
import { displayModelName } from '../thread-events';
import {
  listModels, createModel, updateModel, deleteModelApi, type RouteInput,
} from '../../api/client';
import type { ModelInfo, RouteInfo } from '../../api/types';
import { errorDetail } from '../../utils/errorDetail';

/** Whether `provider` is among the backends the engine actually has configured.
 *  `null` (mock, an older engine, or before the first /health probe) means
 *  "don't filter" — everything counts as configured, so the picker is never
 *  spuriously empty. Drives both the picker filter and the manager's badge. */
export function isProviderConfigured(provider: string): boolean {
  const set = configuredProviders.value;
  return set === null || set.includes(provider);
}

/** Load the DB-backed model registry into the `chatModels` signal. Refetches
 *  (e.g. after a Model* SSE) keep showing existing data through the round trip
 *  and swap atomically — same flash-avoidance as `loadCredentials`. */
export async function loadChatModels(): Promise<void> {
  const firstLoad = chatModels.value.status !== 'loaded';
  setLoadingIfFresh(chatModels);
  try {
    const data = await listModels();
    chatModels.value = { status: 'loaded', data: data.models || [] };
    // The registry is the authority on which tiers a model supports, and it
    // lands AFTER `loadPreferences` has already clamped the effort with the
    // id-shape heuristic. Re-clamp so the picker never keeps displaying a tier
    // the engine would silently snap on the way to the wire. Display only, like
    // `loadPreferences`: the stored preference is left alone, since the model
    // this clamps against may itself change again.
    //
    // First load only. A reload follows a `Model*` event, and every provider
    // pick writes one. Re-clamping then would narrow the account default every
    // draft falls back to, for a pick on one draft.
    if (firstLoad) {
      reasoningEffort.value = clampEffortFor(reasoningEffort.value, currentModel.value);
    }
  } catch (error) {
    chatModels.value = toFailed(error);
  }
}

/** The registry row for `modelId`, or `undefined` when the registry cannot
 *  answer: it has not loaded, it failed, or the id has no row (a saved
 *  `chat_model` naming a deleted model). */
export function modelRow(modelId: string): ModelInfo | undefined {
  const loadable = chatModels.value;
  if (loadable.status !== 'loaded') return undefined;
  return loadable.data.find((m) => m.id === modelId);
}

/** Whether any of this model's backends is configured.
 *
 *  The picker filter. Before routes it asked about the row's ONE provider. A
 *  workspace holding only an Anthropic key was therefore offered the Fable rows
 *  and nothing else from the Claude family. */
export function hasConfiguredRoute(model: ModelInfo): boolean {
  return model.routes.some((r) => isProviderConfigured(r.provider));
}

/** The route that would serve `modelId`, or `undefined` when no row exists.
 *
 *  Mirrors the engine's `resolve_route`. The preferred provider wins when the
 *  row has that route, CONFIGURED OR NOT. A choice is honoured or refused,
 *  never substituted, so the picker shows the parked one rather than a backend
 *  the turn will not reach. Otherwise the first configured route wins, and with
 *  none configured the row's first. */
export function resolvedRoute(modelId: string): RouteInfo | undefined {
  const row = modelRow(modelId);
  if (!row) return undefined;
  const preferred = row.preferred_provider
    ? row.routes.find((r) => r.provider === row.preferred_provider)
    : undefined;
  return preferred ?? row.routes.find((r) => isProviderConfigured(r.provider)) ?? row.routes[0];
}

/** The reasoning efforts the engine says `modelId` supports on the route that
 *  would serve it, or `undefined` when the registry cannot answer. Callers fall
 *  back to the id-shape heuristic in `store/models.ts`. */
export function modelReasoningEfforts(modelId: string): string[] | undefined {
  return resolvedRoute(modelId)?.reasoning_efforts;
}

/** The Lucidos Agent's tier vocabulary, for `useModelSelection`. */
export const LUCIDOS_TIER_VOCABULARY: readonly TierChoice[] = REASONING_LEVELS;

/** Snap an effort onto the closest tier a model supports. Call this on EVERY
 *  model change: an effort left over from the previous model is exactly what
 *  the engine has to clamp at the chokepoint, and a picker showing one value
 *  while the request carries another is the confusion this pairing removes.
 *
 *  Pickers get this through `useModelSelection`, which clamps as part of a
 *  pick. The two remaining direct callers are not pickers: saving the account
 *  model, and re-clamping the displayed effort once the registry lands. */
export function clampEffortFor(effort: string, modelId: string): string {
  const offered = tierOptions(
    lucidosTiers(modelId, modelReasoningEfforts(modelId)),
    LUCIDOS_TIER_VOCABULARY,
  );
  return clampToOffered(effort, offered) ?? effort;
}

/** Every backend that can serve `modelId`, in route order, each with the
 *  efforts it accepts. `undefined` when the registry has no row for it. */
function providerChoices(modelId: string): ProviderChoice[] | undefined {
  return modelRow(modelId)?.routes.map((r) => ({
    value: r.provider,
    label: providerLabel(r.provider),
    configured: isProviderConfigured(r.provider),
    reasoningEfforts: lucidosTiers(r.id, r.reasoning_efforts),
  }));
}

/** One model as the Lucidos Agent's picker offers it. */
function lucidosModelChoice(value: string, label: string): ModelChoice {
  return {
    value,
    label,
    reasoningEfforts: lucidosTiers(value, modelReasoningEfforts(value)),
    providers: providerChoices(value),
    defaultProvider: resolvedRoute(value)?.provider,
  };
}

/** The Lucidos Agent's model rows, each carrying its tiers and its backends.
 *
 *  This is the adapter half of the *model selection* unit: it turns the
 *  registry (or the static fallback) into the shape `useModelSelection` reads,
 *  which is the same shape the coding-agent menu gets off the wire.
 *
 *  `current` is appended when the registry does not list it. A saved
 *  `chat_model` naming a deleted or disabled model then still renders as
 *  selected, rather than the picker silently showing something else. */
export function lucidosModelChoices(current?: string | null): ModelChoice[] {
  const choices = chatModelOptions().map((o) => lucidosModelChoice(o.value, o.label));
  if (current && !choices.some((c) => c.value === current)) {
    choices.push(lucidosModelChoice(current, displayModelName(current)));
  }
  return choices;
}

/** Options for the chat model `<Dropdown>` — enabled models from the loaded
 *  registry with at least one configured route, falling back to the static
 *  `MODELS` list before the first load (so the picker never renders empty).
 *
 *  The filter keeps a user with only an OpenAI key from being offered models
 *  that would error on use. It asks about ANY route rather than one provider.
 *  That is what makes Opus and Sonnet reachable from a workspace whose only
 *  credential is an Anthropic key.
 *
 *  A model whose PREFERRED route is parked stays listed on purpose. Its turn is
 *  refused rather than moved, and the provider step is where that gets fixed.
 *  Hiding the model would be a dead end. */
export function chatModelOptions(): Array<{ value: string; label: string }> {
  const loadable = chatModels.value;
  if (loadable.status === 'loaded') {
    return loadable.data
      .filter((m) => m.enabled && hasConfiguredRoute(m))
      .map((m) => ({ value: m.id, label: m.label }));
  }
  return MODELS;
}

/** Shared success/error/reload handling for model mutations, mirroring
 *  `runCredentialSave`. Returns whether the mutation succeeded. */
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

/** Parse the Add Model form's optional "Context window" field. Blank means "let
 *  the engine infer it from the id" (`undefined`); anything non-numeric or
 *  non-positive is a user error, not a silent fallback — a bad value would
 *  otherwise be dropped and the model would quietly keep the 200k default. */
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
  contextWindow: string
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
      }),
    'Failed to add model'
  );
}

/** Remember the backend last picked for this model, from any picker's
 *  provider step. The user asked for the last pick to be remembered per model.
 *
 *  Per MODEL, on its registry row, rather than one account preference. A global
 *  one means nothing across families that share no backend: picking Grok on xAI
 *  would otherwise move Opus off Anthropic.
 *
 *  So a compose pick writes it too, a deliberate exception to the per-draft rule
 *  (`.claude/rules/frontend.md`). A draft that picked also carries its own
 *  override, so a later pick elsewhere cannot move it. A no-op when the row
 *  already remembers this backend. */
export async function rememberProvider(id: string, provider: string): Promise<void> {
  if (modelRow(id)?.preferred_provider === provider) return;
  await runModelMutation(
    () => updateModel(id, { preferred_provider: provider }),
    'Failed to remember the provider'
  );
}

/** One route as the Settings route editor holds it: raw field text. */
export interface RouteDraft {
  provider: string;
  /** The wire id, blank for the model's own id. */
  id: string;
  /** The window in tokens, blank to infer it from the id. */
  contextWindow: string;
}

/** The editor's route drafts for a model, in priority order. A wire id equal
 *  to the model's own reads blank, since that is the default it spells out. */
export function routeDrafts(model: ModelInfo): RouteDraft[] {
  return model.routes.map((r) => ({
    provider: r.provider,
    id: r.id === model.id ? '' : r.id,
    contextWindow: r.context_window ? String(r.context_window) : '',
  }));
}

/** Turn the editor's drafts into the route list the engine stores, or say
 *  what is wrong. The engine checks the same rules; this names the field. */
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

/** Save a model's routes from the route editor, and its remembered provider
 *  when the user changed it. `undefined` leaves the stored one alone, so a pick
 *  made in a chat picker while the editor was open survives the save. The
 *  engine drops a stored one the new routes no longer serve. */
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

export function setModelEnabled(id: string, enabled: boolean): Promise<boolean> {
  return runModelMutation(() => updateModel(id, { enabled }), 'Failed to update model');
}

export async function deleteModel(id: string): Promise<void> {
  if (!(await showConfirm(`Delete model "${id}"?`, 'Delete'))) {
    return;
  }
  await runModelMutation(() => deleteModelApi(id), 'Failed to delete model');
}
