import { toFailed } from '../../store/types';
import { HEALTH_PROBE_TIMEOUT_MS } from '../../store/store';
import { API, json, mutatingFetch, mutatingFetchIdempotent, throwIfNotOk } from './_core';
import type { DiffFile } from '../../store/store';
import type { AnswerKind, PersistScope } from '../../store/thread-events';
import type { Loadable } from '../../store/types';
import type { ChatRequestBody, CodingAgent } from '../types';

// --- Health ---
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
  /** True in a packaged desktop build (engine serves the bundled frontend and
   *  runs as the launchd service). Routes the "Restart" control: packaged →
   *  restart the LaunchAgent; dev → the /restart rebuild script. */
  packaged?: boolean;
  /** False when the engine booted with no LLM provider configured (the
   *  UnconfiguredProvider sentinel — packaged first run). Drives first-run
   *  provider onboarding. Absent on older engines → treated as configured. */
  llm_configured?: boolean;
  /** Provider backends the engine actually has configured
   *  (`vertex`/`anthropic`/`openai`/`openrouter`/`local`). Used to filter the
   *  model picker to providers the user has set up. `null`/absent = don't filter
   *  (mock, or an older engine). Reflects a runtime credential swap. */
  configured_providers?: string[] | null;
  /** Can the engine reach its own database? An engine outlives its database, so
   *  it keeps answering this endpoint (200, `status: "ok"`) while every query
   *  behind it fails. Absent on older engines, which reads as reachable: only an
   *  explicit `false` puts the client into its degraded surface. ADR 0037. */
  database_reachable?: boolean;
}

/** Probe `/api/v1/health`. Failed without `httpCode` = transport unreachable;
 *  with `httpCode` = engine answered non-2xx. The connection watchdog only
 *  reads `loaded` vs `failed` today, but downstream telemetry / smarter
 *  recovery can branch on the discriminator without another round-trip.
 *
 *  Its own deadline rather than the shared 10s default, because this probe's
 *  latency IS its answer and it has to answer before the next poll: see
 *  `HEALTH_PROBE_TIMEOUT_MS` for why that number is what it is. The service
 *  worker leaves this endpoint alone (`public/sw.js`) so the whole budget
 *  belongs to one attempt. */
export async function checkHealth(): Promise<Loadable<HealthInfo>> {
  try {
    return { status: 'loaded', data: await json<HealthInfo>(`${API}/health`, undefined, HEALTH_PROBE_TIMEOUT_MS) };
  } catch (err) {
    return toFailed<HealthInfo>(err);
  }
}

// --- Chat ---
export async function submitChat(body: ChatRequestBody): Promise<{ event_id: string }> {
  const res = await mutatingFetch(`${API}/chat/stream`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  });
  await throwIfNotOk(res);
  return res.json();
}

/** Parse a `{"canceled": bool}` cancel/stop response. A missing or unparsable
 *  body (older engine, or any 200 with no JSON) defaults to `true` — the
 *  pre-body behavior, where every 200 meant "canceled" — so a legacy engine
 *  never triggers the stale-state heal path. `false` means the server had
 *  nothing to cancel: the caller's view is stale and it should re-sync. */
async function parseCanceled(res: Response): Promise<boolean> {
  const body = await res.json().catch(() => null);
  if (body && typeof body === 'object' && typeof (body as { canceled?: unknown }).canceled === 'boolean') {
    return (body as { canceled: boolean }).canceled;
  }
  return true;
}

/** Cancel a running Lucidos Agent (chat) turn. Returns whether the server
 *  actually canceled something: `false` when nothing was running (the client's
 *  optimistic "canceling" state is stale and must be reconciled). */
export async function cancelChat(threadId?: string): Promise<boolean> {
  const params = threadId ? `?thread_id=${encodeURIComponent(threadId)}` : '';
  const res = await mutatingFetchIdempotent(`${API}/chat/cancel${params}`, { method: 'POST' });
  await throwIfNotOk(res);
  return parseCanceled(res);
}

export async function removeQueuedMessage(threadId: string, messageId: string): Promise<void> {
  const res = await mutatingFetch(`${API}/chat/queued-message/remove`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ thread_id: threadId, message_id: messageId }),
  });
  await throwIfNotOk(res);
}

export async function applyNow(threadId: string): Promise<void> {
  const res = await mutatingFetch(`${API}/claude-code/apply-now?thread_id=${encodeURIComponent(threadId)}`, { method: 'POST' });
  await throwIfNotOk(res);
}

/** Resume an interrupted thread — chat/trigger threads route through
 *  chat::rerun, CC threads through ContinuationRequested. The dispatch is decided
 *  server-side based on thread type. */
export async function continueThread(threadId: string): Promise<void> {
  const res = await mutatingFetch(`${API}/threads/${encodeURIComponent(threadId)}/continue`, { method: 'POST' });
  await throwIfNotOk(res);
}

/** **Stop waiting** on ONE of a thread's live event waits. The thread keeps any
 *  others (a thread-level Stop, which cancels them all, is
 *  `POST /api/v1/chat/cancel`).
 *  404s when the wait already resolved, which is what the row's error toast
 *  reports rather than silently pretending it stopped something. */
export async function cancelThreadEventWait(threadId: string, waitId: string): Promise<void> {
  const res = await mutatingFetch(
    `${API}/threads/${encodeURIComponent(threadId)}/event-waits/${encodeURIComponent(waitId)}/cancel`,
    { method: 'POST' },
  );
  await throwIfNotOk(res);
}

export async function sendControlRequest(threadId: string, request: Record<string, string>): Promise<void> {
  await json(`${API}/claude-code/control`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ thread_id: threadId, request }),
  });
}

export interface CodingAgentCommandOption {
  value: string;
  label: string;
  description: string;
  /** When present, the option is available only for these explicit model ids. */
  supported_models?: string[];
}

export interface CodingAgentCommandParam {
  key: string;
  label?: string;
  placeholder?: string;
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
}

/** Model aliases mirroring the `models` list in crates/lucidos-engine/src/runtime/cc_menu_options.json. */
export type CodingAgentModelValue = 'default' | 'claude-fable-5' | 'claude-fable-5[1m]' | 'claude-sonnet-5' | 'sonnet' | 'claude-opus-5@default' | 'claude-opus-5[1m]' | 'claude-opus-4-8@default' | 'claude-opus-4-8[1m]' | 'claude-opus-4-7' | 'claude-opus-4-1' | 'opus' | 'opus[1m]' | 'haiku';

/** Reasoning effort levels mirroring the `reasoning_efforts` list in crates/lucidos-engine/src/runtime/cc_menu_options.json. */
export type CodingAgentReasoningEffort = 'low' | 'medium' | 'high' | 'xhigh' | 'max';

export async function fetchCodingAgentCommands(
  threadId?: string,
  repoId?: string,
  codingAgent?: CodingAgent,
): Promise<CodingAgentCommandsResponse> {
  const params = new URLSearchParams();
  if (threadId) params.set('thread_id', threadId);
  // Pass empty string explicitly so the backend resolves to the default
  // "Lucidos" repo rather than the legacy no-repo fallback.
  if (repoId !== undefined) params.set('repo_id', repoId);
  // Compose-view only: an existing thread's backend comes from its
  // thread_summaries row server-side, not the client.
  if (!threadId && codingAgent && codingAgent !== 'claude-code') {
    params.set('coding_agent', codingAgent);
  }
  const qs = params.toString();
  return json<CodingAgentCommandsResponse>(`${API}/claude-code/commands${qs ? `?${qs}` : ''}`);
}

/** Stop a running Claude Code session.
 *
 * Three modes:
 *   - default (no `apply`, no `discard`): real Cancel/Stop click. Backend emits
 *     `ResponseCanceled` if CC was actively working; nothing if CC was idle.
 *   - `apply=true`: Apply Now. Backend auto-applies the change after CC stops.
 *     No `ResponseCanceled` — `ChangeApplied` is the terminator.
 *   - `discard=true`: Discard. Backend drops the change after CC stops. No
 *     `ResponseCanceled` — `ChangeDiscarded` is the terminator.
 *
 * Archive uses a different endpoint (`POST /api/v1/threads/archive`) which sets
 * `StopReason::Archive` so `ThreadArchived` is the terminator.
 *
 * Returns whether the server actually stopped something. A default (no
 * apply/discard) Stop that races an already-finished turn returns `false` — the
 * client's optimistic "canceling" state is stale and must be reconciled. Apply
 * / Discard always report `true` (their terminator is `ChangeApplied` /
 * `ChangeDiscarded`, and callers don't read the return).
 */
export async function stopClaudeCode(apply?: boolean, threadId?: string, discard?: boolean): Promise<boolean> {
  const params = new URLSearchParams();
  if (apply) params.set('apply', 'true');
  if (discard) params.set('discard', 'true');
  if (threadId) params.set('thread_id', threadId);
  const qs = params.toString();
  const url = qs ? `${API}/claude-code/stop?${qs}` : `${API}/claude-code/stop`;
  // Idempotent + iOS PWA retry: HTTP/2 half-closed POSTs after backgrounding
  // reject with TypeError("Load failed"), forcing the user to click again.
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

/** Answer a pending question card on a thread. The backend emits
 *  UserQuestionAnswered and dispatches the agent-specific resume side
 *  effects: for CC threads it respawns the subprocess with --resume + a
 *  matching tool_result; for chat threads it wakes the in-process
 *  `ask_user_question` tool which returns the answer as a tool_result on
 *  the same turn. Returns true on success; false for 409 (stale/duplicate)
 *  so the UI can re-sync from events. */
export async function answerThreadQuestion(
  threadId: string,
  toolUseId: string,
  answer: AnswerKind,
): Promise<boolean> {
  const res = await mutatingFetch(`${API}/threads/${encodeURIComponent(threadId)}/answer-question`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ tool_use_id: toolUseId, answer }),
  });
  if (res.ok) return true;
  if (res.status === 409) return false; // already answered or no pending question
  await throwIfNotOk(res);
  throw new Error('unreachable');
}

// --- MCP Consent ---

export async function postMcpConsent(
  requestId: string,
  allowed: boolean,
  persistScope?: PersistScope,
): Promise<void> {
  const body: Record<string, unknown> = { request_id: requestId, allowed };
  if (persistScope) body.persist_scope = persistScope;
  const resp = await mutatingFetch(`${API}/mcp/consent`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  });
  await throwIfNotOk(resp);
}

// --- Command-guard consent (ADR 0002) — chat mirror of MCP consent ---

export async function postCommandConsent(
  requestId: string,
  allowed: boolean,
  persistScope?: PersistScope,
): Promise<void> {
  const body: Record<string, unknown> = { request_id: requestId, allowed };
  if (persistScope) body.persist_scope = persistScope;
  const resp = await mutatingFetch(`${API}/command-permission/consent`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  });
  await throwIfNotOk(resp);
}

// --- Chat MCP permission consent — chat mirror of MCP consent for MCP tools ---

export async function postMcpPermissionConsent(
  requestId: string,
  allowed: boolean,
  persistScope?: PersistScope,
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

// --- Command-guard checkpoint undo (ADR 0002, Phase 4) ---

export async function postCommandCheckpointUndo(checkpointId: string): Promise<void> {
  const resp = await mutatingFetch(`${API}/command-checkpoint/undo`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ checkpoint_id: checkpointId }),
  });
  await throwIfNotOk(resp);
}

/** What the checkpointed command changed, as the diff between the snapshot
 *  taken before it ran and the one taken after. `reclaimed` is the pair being
 *  gone (aged out of the 30-day retention, or a checkpoint predating the post
 *  image), which the modal explains instead of rendering an empty diff. */
export async function getCommandCheckpointDiff(
  checkpointId: string,
): Promise<{ files: DiffFile[]; reclaimed: boolean }> {
  return json(`${API}/command-checkpoint/diff?checkpoint_id=${encodeURIComponent(checkpointId)}`);
}

// --- CC allowed tools (~/.lucidos/cc-allowed-tools) ---
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

// --- Lucidos Agent command allowlist (~/.lucidos/agent-allowed-commands, ADR 0002) ---
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
