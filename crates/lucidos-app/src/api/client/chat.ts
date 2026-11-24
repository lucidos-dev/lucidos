import { remoteConnectionFailure } from '../../utils/remoteConnection';
import { toFailed } from '../../store/types';
import { HEALTH_PROBE_TIMEOUT_MS } from '../../store/store';
import { API, ApiError, json, mutatingFetch, mutatingFetchIdempotent, throwIfNotOk } from './_core';
import type { DiffFile } from '../../store/store';
import type { AnswerKind, AllowScope } from '../../store/thread-events';
import type { Loadable } from '../../store/types';
import type { Band, ChatRequestBody, CodingAgent } from '../types';
import { SIDE_QUESTION_TIMEOUT_MS } from '@lucidos/engine-constants';
import type { ComposeReportWire } from '../../store/composeReport';

export interface HealthInfo {
  status: string;
  workspace: string;
  workspace_path: string;
  started_at: string;
  release?: string;
  release_dirty?: boolean;
  engine_version?: string;
  latest_engine_version?: string;
  latest_tauri_app_version?: string;
  packaged?: boolean;
  llm_configured?: boolean;
  configured_providers?: string[] | null;
  database_reachable?: boolean;
  database_pool_exhausted?: boolean;
  agent_sandbox_unavailable?: string | null;
  agent_sandbox_truncation_unguarded?: boolean;
  agent_sandbox_metadata_unguarded?: boolean;
}

export type HealthProbe = Loadable<HealthInfo> | { status: 'stopped' };

export async function checkHealth(): Promise<HealthProbe> {
  try {
    const data = await json<HealthInfo>(`${API}/health`, undefined, HEALTH_PROBE_TIMEOUT_MS);
    remoteConnectionFailure.value = null;
    return { status: 'loaded', data };
  } catch (err) {
    remoteConnectionFailure.value = null;
    if (err instanceof ApiError && err.body && typeof err.body === 'object' && 'remote_gateway' in err.body && 'reason_kind' in err.body) {
      remoteConnectionFailure.value = { reason: err.reason, reason_kind: String(err.body.reason_kind) };
    }
    if (err instanceof ApiError && err.workspaceStopped) return { status: 'stopped' };
    return toFailed<HealthInfo>(err);
  }
}

export async function getWorkspaceLabel(): Promise<string | null> {
  const body = await json<{ label: string | null }>(`${API}/workspace-label`);
  return body.label ?? null;
}

export const SUBMIT_CHAT_TIMEOUT_MS = 20_000;

export interface ChatSubmitAck {
  event_id: string;
  compose?: ComposeReportWire;
}

export async function submitChat(body: ChatRequestBody): Promise<ChatSubmitAck> {
  const res = await mutatingFetch(`${API}/chat/stream`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
    timeoutMs: SUBMIT_CHAT_TIMEOUT_MS,
  });
  await throwIfNotOk(res);
  return res.json();
}

async function parseCanceled(res: Response): Promise<boolean> {
  const body = await res.json().catch(() => null);
  if (body && typeof body === 'object' && typeof (body as { canceled?: unknown }).canceled === 'boolean') {
    return (body as { canceled: boolean }).canceled;
  }
  return true;
}

export interface ChatCancelResponse {
  canceled: boolean;
  taken_back_messages: Array<{ thread_id: string; id: string; text: string; image_hashes: string[]; compose: ComposeReportWire }>;
  failed_take_back_ids: string[];
}

export async function cancelChat(threadId?: string): Promise<ChatCancelResponse> {
  const params = threadId ? `?thread_id=${encodeURIComponent(threadId)}` : '';
  const res = await mutatingFetch(`${API}/chat/cancel${params}`, { method: 'POST' });
  await throwIfNotOk(res);
  const text = await res.text();
  const body = text.trim() ? JSON.parse(text) : null;
  return {
    canceled: typeof body?.canceled === 'boolean' ? body.canceled : true,
    taken_back_messages: body?.taken_back_messages ?? [],
    failed_take_back_ids: body?.failed_take_back_ids ?? [],
  };
}

export async function removeQueuedMessage(threadId: string, messageId: string): Promise<ComposeReportWire | null> {
  const res = await mutatingFetch(`${API}/chat/queued-message/remove`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ thread_id: threadId, message_id: messageId }),
  });
  await throwIfNotOk(res);
  const text = await res.text();
  return text.trim() ? (JSON.parse(text).compose ?? null) : null;
}

export interface ChatSettingsPick {
  model: string;
  reasoningEffort?: string;
  provider?: string;
}

export async function recordChatSettings(threadId: string, pick: ChatSettingsPick): Promise<void> {
  const res = await mutatingFetch(`${API}/chat/settings`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      thread_id: threadId,
      model: pick.model,
      reasoning_effort: pick.reasoningEffort,
      provider: pick.provider,
    }),
  });
  await throwIfNotOk(res);
}

export async function applyNow(threadId: string): Promise<{ summary_version?: number | null }> {
  const res = await mutatingFetch(`${API}/claude-code/apply-now?thread_id=${encodeURIComponent(threadId)}`, { method: 'POST' });
  await throwIfNotOk(res);
  return res.json();
}

export async function continueThread(threadId: string): Promise<void> {
  const res = await mutatingFetch(`${API}/threads/${encodeURIComponent(threadId)}/continue`, { method: 'POST' });
  await throwIfNotOk(res);
}

export async function hardenThread(threadId: string): Promise<void> {
  const res = await mutatingFetch(`${API}/threads/${encodeURIComponent(threadId)}/harden`, { method: 'POST' });
  await throwIfNotOk(res);
}

export async function holdThread(threadId: string, reason: string): Promise<{ held: boolean; changed: boolean }> {
  return json(`${API}/threads/${encodeURIComponent(threadId)}/hold`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ reason }),
  });
}

export async function resumeThread(threadId: string): Promise<{ held: boolean; changed: boolean }> {
  return json(`${API}/threads/${encodeURIComponent(threadId)}/hold`, { method: 'DELETE' });
}

export async function cancelThreadEventWait(threadId: string, waitId: string): Promise<void> {
  const res = await mutatingFetch(
    `${API}/threads/${encodeURIComponent(threadId)}/event-waits/${encodeURIComponent(waitId)}/cancel`,
    { method: 'POST' },
  );
  await throwIfNotOk(res);
}

export interface ControlResponse {
  ok: true;
}

export async function sendControlRequest(threadId: string, request: Record<string, string>): Promise<ControlResponse> {
  return json<ControlResponse>(`${API}/claude-code/control`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ thread_id: threadId, request }),
  });
}

const SIDE_QUESTION_CLIENT_MARGIN_MS = 10_000;
const SIDE_QUESTION_CLIENT_WAIT_MS = SIDE_QUESTION_TIMEOUT_MS + SIDE_QUESTION_CLIENT_MARGIN_MS;

export async function askSideQuestion(
  threadId: string,
  sideQuestionId: string,
  question: string,
  imageHashes: readonly string[],
): Promise<string> {
  const body = await json<{ answer: string }>(`${API}/side-questions`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      thread_id: threadId, side_question_id: sideQuestionId, question, image_hashes: imageHashes,
    }),
  }, SIDE_QUESTION_CLIENT_WAIT_MS);
  return body.answer;
}

export async function dismissSideQuestion(threadId: string, sideQuestionId: string): Promise<void> {
  await json(`${API}/side-questions/dismiss`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ thread_id: threadId, side_question_id: sideQuestionId }),
  });
}

export interface CodingAgentCommandOption {
  value: string;
  resolved_model?: string | null;
  label: string;
  description: string;
  reasoning_efforts?: string[];
}

export interface CodingAgentCommandParam {
  key: string;
  label?: string;
  options?: CodingAgentCommandOption[];
}

export interface CodingAgentCommandDef {
  subtype: string;
  label: string;
  params: CodingAgentCommandParam[];
}

export interface CodingAgentCommandsResponse {
  control_commands: CodingAgentCommandDef[];
  builtin_commands: string[];
  skill_commands: string[];
  current_model: string | null;
  current_reasoning_effort: string | null;
  has_active_session: boolean;
  default_implementer?: { model: string; effort: string | null } | null;
  ladder_rungs?: CodingAgentLadderRung[];
  models_provenance?: CodingAgentModelsProvenance;
}

export interface CodingAgentLadderRung {
  model: string;
  effort: string;
  label: string;
  estimated: boolean;
  intelligence: Band | null;
  cost: Band | null;
  time: Band | null;
}

export interface CodingAgentModelsProvenance {
  source: 'discovered' | 'fallback';
  discovered_at: string | null;
  cc_version: string | null;
  error: string | null;
}

export type CodingAgentModelValue = string;

export type CodingAgentReasoningEffort = 'low' | 'medium' | 'high' | 'xhigh' | 'max';

export async function fetchCodingAgentCommands(
  threadId?: string,
  repoId?: string,
  codingAgent?: CodingAgent,
): Promise<CodingAgentCommandsResponse> {
  const params = new URLSearchParams();
  if (threadId) params.set('thread_id', threadId);
  if (repoId !== undefined) params.set('repo_id', repoId);
  if (!threadId && codingAgent && codingAgent !== 'claude-code') {
    params.set('coding_agent', codingAgent);
  }
  const qs = params.toString();
  return json<CodingAgentCommandsResponse>(`${API}/claude-code/commands${qs ? `?${qs}` : ''}`);
}

export async function stopClaudeCode(apply?: boolean, threadId?: string, discard?: boolean): Promise<boolean> {
  const params = new URLSearchParams();
  if (apply) params.set('apply', 'true');
  if (discard) params.set('discard', 'true');
  if (threadId) params.set('thread_id', threadId);
  const qs = params.toString();
  const url = qs ? `${API}/claude-code/stop?${qs}` : `${API}/claude-code/stop`;
  const res = await mutatingFetchIdempotent(url, { method: 'POST' });
  await throwIfNotOk(res);
  return parseCanceled(res);
}

export async function discardCCChanges(threadId: string): Promise<void> {
  const res = await mutatingFetch(`${API}/claude-code/discard`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ thread_id: threadId }),
  });
  await throwIfNotOk(res);
}

export interface AnswerPlacement {
  event_id: string;
  input_chain: string;
  input_seq: number;
  compose_epoch?: number;
}

export async function answerThreadQuestion(
  threadId: string,
  toolUseId: string,
  answer: AnswerKind,
  placement: AnswerPlacement,
  acceptedCompose?: (report: ComposeReportWire | null | undefined) => void,
): Promise<boolean> {
  const res = await mutatingFetch(`${API}/threads/${encodeURIComponent(threadId)}/answer-question`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ tool_use_id: toolUseId, answer, ...placement }),
    timeoutMs: SUBMIT_CHAT_TIMEOUT_MS,
  });
  if (res.ok) {
    if (acceptedCompose) acceptedCompose((await res.json() as { compose?: ComposeReportWire | null }).compose);
    return true;
  }
  if (res.status === 409) {
    const refusal = await res.clone().json() as { reason?: string };
    if (['compose_snapshot_stale', 'compose_fence_required', 'overtaken'].includes(refusal.reason ?? '')) {
      await throwIfNotOk(res);
    }
  }
  if (res.status === 409) return false;
  await throwIfNotOk(res);
  throw new Error('unreachable');
}


export async function postMcpConsent(
  requestId: string,
  allowed: boolean,
  persistScope?: AllowScope,
  alwaysDeny = false,
): Promise<void> {
  const body: Record<string, unknown> = { request_id: requestId, allowed };
  if (persistScope) body.persist_scope = persistScope;
  if (alwaysDeny) body.always_deny = true;
  const resp = await mutatingFetch(`${API}/mcp/consent`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  });
  await throwIfNotOk(resp);
}


export async function postCommandConsent(
  requestId: string,
  allowed: boolean,
  persistScope?: AllowScope,
  alwaysDeny = false,
): Promise<void> {
  const body: Record<string, unknown> = { request_id: requestId, allowed };
  if (persistScope) body.persist_scope = persistScope;
  if (alwaysDeny) body.always_deny = true;
  const resp = await mutatingFetch(`${API}/command-permission/consent`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  });
  await throwIfNotOk(resp);
}


export async function postMcpPermissionConsent(
  requestId: string,
  allowed: boolean,
  persistScope?: AllowScope,
): Promise<void> {
  const body: Record<string, unknown> = { request_id: requestId, allowed };
  if (persistScope) body.persist_scope = persistScope;
  const resp = await mutatingFetch(`${API}/mcp-permission/consent`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  });
  await throwIfNotOk(resp);
}


export async function postCommandCheckpointUndo(checkpointId: string): Promise<void> {
  const resp = await mutatingFetch(`${API}/command-checkpoint/undo`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ checkpoint_id: checkpointId }),
  });
  await throwIfNotOk(resp);
}

export async function getCommandCheckpointDiff(
  checkpointId: string,
): Promise<{ files: DiffFile[]; reclaimed: boolean }> {
  return json(`${API}/command-checkpoint/diff?checkpoint_id=${encodeURIComponent(checkpointId)}`);
}

export async function getCcAllowedTools(): Promise<string> {
  const body = await json<{ contents: string }>(`${API}/cc-allowed-tools`);
  return body.contents;
}

export async function putCcAllowedTools(contents: string): Promise<void> {
  const resp = await mutatingFetch(`${API}/cc-allowed-tools`, {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ contents }),
  });
  await throwIfNotOk(resp);
}

export async function getAgentDeniedCommands(): Promise<string> {
  const body = await json<{ contents: string }>(`${API}/agent-denied-commands`);
  return body.contents;
}

export async function putAgentDeniedCommands(contents: string): Promise<void> {
  const resp = await mutatingFetch(`${API}/agent-denied-commands`, {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ contents }),
  });
  await throwIfNotOk(resp);
}

export async function getAgentAllowedCommands(): Promise<string> {
  const body = await json<{ contents: string }>(`${API}/agent-allowed-commands`);
  return body.contents;
}

export async function putAgentAllowedCommands(contents: string): Promise<void> {
  const resp = await mutatingFetch(`${API}/agent-allowed-commands`, {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ contents }),
  });
  await throwIfNotOk(resp);
}
