
import { signal } from '@preact/signals';
import {
  type Scope,
  selectedScope,
  selectedCodingAgent,
  threadMap,
  preferences,
  currentModel,
} from './store';
import { accountChatEffort } from './actions/preferences';
import type { CodingAgent } from '../api/types';
import type { CodingAgentModelValue, CodingAgentReasoningEffort } from '../api/client';

export interface ComposeSelectionOverride {
  scope?: Scope;
  codingAgent?: CodingAgent;
  model?: string;
  reasoningEffort?: string;
  provider?: string;
  ccModel?: CodingAgentModelValue | null;
  ccReasoningEffort?: CodingAgentReasoningEffort | null;
  sideQuestionMode?: boolean;
  user?: string | null;
}

export const composeSelections = signal<Map<string, ComposeSelectionOverride>>(new Map());

export const pendingComposeSelection = signal<ComposeSelectionOverride>({});

const EMPTY_OVERRIDE: ComposeSelectionOverride = Object.freeze({});

export function getComposeSelectionOverride(
  threadId: string | null | undefined,
): ComposeSelectionOverride {
  if (!threadId) return pendingComposeSelection.value;
  return composeSelections.value.get(threadId) ?? EMPTY_OVERRIDE;
}

export function patchComposeSelection(
  threadId: string | null | undefined,
  patch: ComposeSelectionOverride,
): void {
  if (!threadId) {
    pendingComposeSelection.value = { ...pendingComposeSelection.value, ...patch };
    return;
  }
  const prev = composeSelections.value.get(threadId) ?? EMPTY_OVERRIDE;
  const next: ComposeSelectionOverride = { ...prev, ...patch };
  const map = new Map(composeSelections.value);
  map.set(threadId, next);
  composeSelections.value = map;
}

export function clearComposeSelection(threadId: string | null | undefined): void {
  if (!threadId) {
    if (Object.keys(pendingComposeSelection.value).length > 0) pendingComposeSelection.value = {};
    return;
  }
  if (!composeSelections.value.has(threadId)) return;
  const map = new Map(composeSelections.value);
  map.delete(threadId);
  composeSelections.value = map;
}

export function takePendingComposeSelection(): ComposeSelectionOverride {
  const pending = pendingComposeSelection.value;
  if (Object.keys(pending).length > 0) pendingComposeSelection.value = {};
  return pending;
}

export function seedComposeSelection(threadId: string, override: ComposeSelectionOverride): void {
  if (Object.keys(override).length === 0) return;
  const map = new Map(composeSelections.value);
  map.set(threadId, { ...map.get(threadId), ...override });
  composeSelections.value = map;
}

export function setComposeSelectionFromServer(
  threadId: string,
  override: ComposeSelectionOverride | null | undefined,
): void {
  const has = composeSelections.value.has(threadId);
  const isEmpty = !override || Object.keys(override).length === 0;
  if (isEmpty) {
    if (!has) return;
    const map = new Map(composeSelections.value);
    map.delete(threadId);
    composeSelections.value = map;
    return;
  }
  const map = new Map(composeSelections.value);
  map.set(threadId, { ...override });
  composeSelections.value = map;
}


export function resolveScope(threadId: string | null | undefined): Scope {
  const override = getComposeSelectionOverride(threadId).scope;
  if (override) return override;
  return threadId ? { kind: 'lucidos' } : selectedScope.value;
}

export function resolveUser(threadId: string | null | undefined): string | null {
  return getComposeSelectionOverride(threadId).user ?? null;
}

export function applyCodingAgentDefault(agent: CodingAgent): void {
  if (agent === selectedCodingAgent.value) return;
  for (const [id, thread] of threadMap.value) {
    if (preferences.value.status === 'loaded' && thread.meta.state === 'composing' && !getComposeSelectionOverride(id).codingAgent) {
      patchComposeSelection(id, { codingAgent: selectedCodingAgent.value });
    }
  }
  selectedCodingAgent.value = agent;
}

export function resolveCodingAgent(threadId: string | null | undefined): CodingAgent {
  return getComposeSelectionOverride(threadId).codingAgent ?? selectedCodingAgent.value;
}

export function resolveModel(threadId: string | null | undefined): string {
  return getComposeSelectionOverride(threadId).model ?? currentModel.value;
}

export function resolveReasoningEffort(threadId: string | null | undefined): string | null {
  return getComposeSelectionOverride(threadId).reasoningEffort ?? accountChatEffort(resolveModel(threadId));
}

export function resolveProvider(threadId: string | null | undefined): string | null {
  return getComposeSelectionOverride(threadId).provider ?? null;
}

export function resolveCcModel(threadId: string | null | undefined): CodingAgentModelValue | null {
  return getComposeSelectionOverride(threadId).ccModel ?? null;
}

export function resolveCcReasoningEffort(
  threadId: string | null | undefined,
): CodingAgentReasoningEffort | null {
  return getComposeSelectionOverride(threadId).ccReasoningEffort ?? null;
}

export function _resetComposeSelectionsForTesting(): void {
  composeSelections.value = new Map();
  pendingComposeSelection.value = {};
}
