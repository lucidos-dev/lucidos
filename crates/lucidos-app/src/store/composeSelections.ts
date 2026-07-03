/** Per-thread compose dropdown selections — target scope, coding-agent backend,
 *  Lucidos Agent model + reasoning, and coding-agent model + reasoning — kept OUT
 *  of the global picker signals so a change while editing one draft never leaks
 *  to another.
 *
 *  SERVER-SYNCED (DB-backed). Unlike its first iteration (client-only, wiped on
 *  reload), a draft's selection is persisted alongside the draft's text/images/
 *  mode in `thread_summaries.compose_selection` and rehydrated on reload and via
 *  the `ThreadComposeChanged` SSE (`setComposeSelectionFromServer`), so a stored
 *  draft keeps its picks across a refresh and across devices. The write path
 *  piggybacks on the debounced compose PUT (`compose.ts` includes the selection
 *  in the body). See docs/plans/2026-07-01-compose-selection-db-persistence.md.
 *
 *  The map stores only OVERRIDES. A field the user hasn't touched for a draft
 *  falls back to the account default at read time — the same
 *  "per-draft value ?? account default" shape as `effectiveSendMode`. Editing a
 *  draft writes ONLY the override (and, for scope, the localStorage last-used
 *  seed below); it never writes the account-wide preference (`chat_model` /
 *  `chat_reasoning_effort` / `coding_agent_default`). `sendCompose` consumes the
 *  resolved values at send and then `clearComposeSelection`s the entry.
 *
 *  SCOPE'S LAST-USED SEED (localStorage). Scope has no account preference — its
 *  default IS the last-used value, `selectedScope` (localStorage). A compose
 *  scope pick updates `selectedScope` so the NEXT new draft seeds from it, and a
 *  new draft eager-copies `selectedScope` into its OWN stored scope at creation
 *  (`ensureFocusedComposeThread`). Crucially, `resolveScope` reads `selectedScope`
 *  ONLY for the no-draft compose view — an EXISTING draft resolves scope from its
 *  own stored value (never the shared `selectedScope`). That is what keeps a scope
 *  pick on draft A from leaking into draft B (the original per-draft bug): no
 *  existing draft reads the shared seed. Model/agent/reasoning have real account
 *  defaults, so reading those as the fallback for an unset field never leaks.
 *
 *  THE PENDING SLOT. A compose draft is created lazily (on the first keystroke),
 *  so the fresh compose view can have NO focused draft yet while the user picks a
 *  destination/agent/model. Those picks (threadId `null`) go to
 *  `pendingComposeSelection`, a single unkeyed override transferred onto the
 *  draft's own entry when it is created (`ensureFocusedComposeThread` →
 *  `seedComposeSelection` + `takePendingComposeSelection`), then cleared. A
 *  `null`/`undefined` threadId routes reads AND writes through the pending slot; a
 *  real id routes through the keyed map. */

import { signal } from '@preact/signals';
import {
  type Scope,
  selectedScope,
  selectedCodingAgent,
  currentModel,
  reasoningEffort,
} from './store';
import type { CodingAgent } from '../api/types';
import type { CodingAgentModelValue, CodingAgentReasoningEffort } from '../api/client';

export interface ComposeSelectionOverride {
  /** Target: Lucidos source / an app / an external repo. */
  scope?: Scope;
  /** Coding-agent backend (Claude Code | Codex). */
  codingAgent?: CodingAgent;
  /** Lucidos Agent model id (the compose mirror of `chat_model`). */
  model?: string;
  /** Lucidos Agent reasoning effort. */
  reasoningEffort?: string;
  /** Coding-agent model; `null` = an explicit "default" pick (distinct from
   *  absent, which falls back to the global). */
  ccModel?: CodingAgentModelValue | null;
  /** Coding-agent reasoning effort. */
  ccReasoningEffort?: CodingAgentReasoningEffort | null;
}

export const composeSelections = signal<Map<string, ComposeSelectionOverride>>(new Map());

/** The not-yet-created draft's picks (fresh compose view, no focused draft).
 *  Kept separate from the account-default globals so a fresh-compose pick never
 *  writes a global that existing drafts read — transferred onto the draft at
 *  creation. */
export const pendingComposeSelection = signal<ComposeSelectionOverride>({});

/** Stable empty override so read-then-fallback callers can't accidentally write
 *  a shared default. */
const EMPTY_OVERRIDE: ComposeSelectionOverride = Object.freeze({});

export function getComposeSelectionOverride(
  threadId: string | null | undefined,
): ComposeSelectionOverride {
  // No draft yet → the pending slot IS this compose context's override.
  if (!threadId) return pendingComposeSelection.value;
  return composeSelections.value.get(threadId) ?? EMPTY_OVERRIDE;
}

/** Patch a compose selection override. A real `threadId` writes the keyed map; a
 *  `null`/`undefined` id (no draft yet) writes the pending slot — NEVER a global.
 *  Callers pass only the fields they mean to set — use `null` (not `undefined`)
 *  to record an explicit empty `ccModel`/`ccReasoningEffort`. Returns a new
 *  Map/object so signal subscribers re-render. */
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

/** Drop a compose selection entirely; subsequent resolves return the current
 *  global defaults. A real id clears its keyed entry (called by `sendCompose`
 *  after a send + the discard path); a `null`/`undefined` id clears the pending
 *  slot. */
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

/** Return the pending slot and clear it — used to transfer a fresh-compose pick
 *  onto the draft that's just been created. */
export function takePendingComposeSelection(): ComposeSelectionOverride {
  const pending = pendingComposeSelection.value;
  if (Object.keys(pending).length > 0) pendingComposeSelection.value = {};
  return pending;
}

/** Seed a newly-created draft's override from a transferred pending selection.
 *  No-op for an empty override so a plain draft keeps falling back to defaults. */
export function seedComposeSelection(threadId: string, override: ComposeSelectionOverride): void {
  if (Object.keys(override).length === 0) return;
  const map = new Map(composeSelections.value);
  map.set(threadId, { ...map.get(threadId), ...override });
  composeSelections.value = map;
}

/** Replace a draft's override wholesale from the server (thread load or SSE
 *  `ThreadComposeChanged`). The DB is the authoritative store, so this REPLACES
 *  (not merges) — a `null`/empty payload means "no per-draft selection stored"
 *  and clears the local entry. Callers apply the same origin/focus/in-flight
 *  guards they use for text before invoking, so this never clobbers a local
 *  in-flight pick. Returns without churning the signal when nothing changes, so
 *  a bulk reload that re-confirms existing selections doesn't re-render. */
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

// --- Resolvers: per-draft override ?? current global default ---

export function resolveScope(threadId: string | null | undefined): Scope {
  const override = getComposeSelectionOverride(threadId).scope;
  if (override) return override;
  // An EXISTING draft must NOT fall back to the shared `selectedScope` — that
  // is the leak the per-draft design fixed (a pick on draft A would move every
  // scope-less draft). New drafts eager-seed their own scope from `selectedScope`
  // at creation, so a real id resolves its own stored scope or the fixed default.
  // Only the no-draft compose view (null/undefined) reads the last-used seed.
  return threadId ? { kind: 'lucidos' } : selectedScope.value;
}

export function resolveCodingAgent(threadId: string | null | undefined): CodingAgent {
  return getComposeSelectionOverride(threadId).codingAgent ?? selectedCodingAgent.value;
}

export function resolveModel(threadId: string | null | undefined): string {
  return getComposeSelectionOverride(threadId).model ?? currentModel.value;
}

export function resolveReasoningEffort(threadId: string | null | undefined): string {
  return getComposeSelectionOverride(threadId).reasoningEffort ?? reasoningEffort.value;
}

// Coding-agent model/effort have no account-wide default (they're per-thread on
// the backend), so compose resolves purely to THIS draft's override or "no pick"
// (null → the send omits the field and the backend applies its own default).
// Deliberately does NOT read the global `codingAgentPending*` — that is the
// active-thread pending, and inheriting it would leak an active thread's mid-
// session pick into a fresh draft.
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
