import {
  chatModels, configuredProviders, currentModel, reasoningEffort, showToast, showConfirm,
} from '../store';
import { toFailed, setLoadingIfFresh } from '../types';
import { MODELS, REASONING_LEVELS } from '../models';
import {
  clampToOffered, lucidosTiers, tierOptions, type ModelChoice, type TierChoice,
} from '../modelSelection';
import { displayModelName } from '../thread-events';
import { listModels, createModel, updateModel, deleteModelApi } from '../../api/client';
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
    reasoningEffort.value = clampEffortFor(reasoningEffort.value, currentModel.value);
  } catch (error) {
    chatModels.value = toFailed(error);
  }
}

/** The reasoning tiers the engine says `modelId` supports, or `undefined` when
 *  the registry cannot answer: it has not loaded (or failed), the id has no row
 *  (a saved `chat_model` for a deleted model), or the engine predates the
 *  field. Callers fall back to the id-shape heuristic in `store/models.ts`. */
export function modelReasoningEfforts(modelId: string): string[] | undefined {
  const loadable = chatModels.value;
  if (loadable.status !== 'loaded') return undefined;
  return loadable.data.find((m) => m.id === modelId)?.reasoning_efforts;
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

/** The Lucidos Agent's model rows, each carrying the tiers it offers.
 *
 *  This is the adapter half of the *model selection* unit: it turns the
 *  registry (or the static fallback) into the shape `useModelSelection` reads,
 *  which is the same shape the coding-agent menu gets off the wire.
 *
 *  `current` is appended when the registry does not list it. A saved
 *  `chat_model` naming a deleted or disabled model then still renders as
 *  selected, rather than the picker silently showing something else. */
export function lucidosModelChoices(current?: string | null): ModelChoice[] {
  const choices = chatModelOptions().map((o) => ({
    value: o.value,
    label: o.label,
    reasoningEfforts: lucidosTiers(o.value, modelReasoningEfforts(o.value)),
  }));
  if (current && !choices.some((c) => c.value === current)) {
    choices.push({
      value: current,
      label: displayModelName(current),
      reasoningEfforts: lucidosTiers(current, modelReasoningEfforts(current)),
    });
  }
  return choices;
}

/** Options for the chat model `<Dropdown>` — enabled models from the loaded
 *  registry whose provider is actually configured, falling back to the static
 *  `MODELS` list before the first load (so the picker never renders empty). The
 *  provider filter keeps a user with only an OpenAI key from being offered
 *  Vertex/Anthropic models that would error on use. */
export function chatModelOptions(): Array<{ value: string; label: string }> {
  const loadable = chatModels.value;
  if (loadable.status === 'loaded') {
    return loadable.data
      .filter((m) => m.enabled && isProviderConfigured(m.provider))
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

export function setModelEnabled(id: string, enabled: boolean): Promise<boolean> {
  return runModelMutation(() => updateModel(id, { enabled }), 'Failed to update model');
}

export async function deleteModel(id: string): Promise<void> {
  if (!(await showConfirm(`Delete model "${id}"?`, 'Delete'))) {
    return;
  }
  await runModelMutation(() => deleteModelApi(id), 'Failed to delete model');
}
