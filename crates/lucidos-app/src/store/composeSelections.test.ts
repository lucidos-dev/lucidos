/** Per-draft compose selections must be independent across drafts and must
 *  never write the global picker signals / account defaults. These tests pin
 *  both: the correctness invariant (editing draft A never changes what draft B
 *  resolves) and the "draft-only, seed from default" invariant (an edit leaves
 *  every global untouched; an un-set draft resolves to the current global). */
import { afterEach, describe, expect, it } from 'vitest';
import {
  _resetComposeSelectionsForTesting,
  clearComposeSelection,
  getComposeSelectionOverride,
  patchComposeSelection,
  pendingComposeSelection,
  resolveCcModel,
  resolveCcReasoningEffort,
  resolveCodingAgent,
  resolveModel,
  resolveReasoningEffort,
  resolveScope,
  seedComposeSelection,
  setComposeSelectionFromServer,
  takePendingComposeSelection,
} from './composeSelections';
import {
  codingAgentPendingModel,
  codingAgentPendingReasoningEffort,
  currentModel,
  reasoningEffort,
  selectedCodingAgent,
  selectedScope,
} from './store';

afterEach(() => {
  _resetComposeSelectionsForTesting();
  // Restore globals to their module defaults so tests don't bleed into each other.
  selectedScope.value = { kind: 'lucidos' };
  selectedCodingAgent.value = 'claude-code';
  codingAgentPendingModel.value = null;
  codingAgentPendingReasoningEffort.value = null;
});

describe('resolve* falls back to the current global default', () => {
  it('returns the live global when no override is stored', () => {
    selectedCodingAgent.value = 'codex';
    currentModel.value = 'model-x';
    reasoningEffort.value = 'medium';
    expect(resolveCodingAgent('t-1')).toBe('codex');
    expect(resolveModel('t-1')).toBe('model-x');
    expect(resolveReasoningEffort('t-1')).toBe('medium');
    expect(resolveScope('t-1')).toEqual({ kind: 'lucidos' });
  });

  it('treats a null/undefined threadId as "no override"', () => {
    currentModel.value = 'model-y';
    expect(resolveModel(null)).toBe('model-y');
    expect(resolveModel(undefined)).toBe('model-y');
  });
});

describe('resolveScope: real drafts never read the shared selectedScope (no-leak); the no-draft view reads the last-used seed', () => {
  it('a scope-less EXISTING draft resolves the fixed {lucidos} default, NOT selectedScope', () => {
    // selectedScope is the last-used seed for NEW drafts only. An existing draft
    // reading it is exactly the leak the per-draft design forbids.
    selectedScope.value = { kind: 'app', appId: 'seed-app' };
    expect(resolveScope('existing-draft')).toEqual({ kind: 'lucidos' });
  });

  it('the no-draft (null) compose view resolves selectedScope (the last-used seed)', () => {
    selectedScope.value = { kind: 'external', repoId: 'seed-repo' };
    expect(resolveScope(null)).toEqual({ kind: 'external', repoId: 'seed-repo' });
    expect(resolveScope(undefined)).toEqual({ kind: 'external', repoId: 'seed-repo' });
  });

  it("a draft's OWN stored scope wins over both the seed and the default", () => {
    selectedScope.value = { kind: 'app', appId: 'seed-app' };
    patchComposeSelection('t-1', { scope: { kind: 'external', repoId: 'r-own' } });
    expect(resolveScope('t-1')).toEqual({ kind: 'external', repoId: 'r-own' });
    // A pending pick on the no-draft view still moves only the no-draft view.
    patchComposeSelection(null, { scope: { kind: 'app', appId: 'pending-app' } });
    expect(resolveScope(null)).toEqual({ kind: 'app', appId: 'pending-app' });
    expect(resolveScope('t-1')).toEqual({ kind: 'external', repoId: 'r-own' });
  });
});

describe('setComposeSelectionFromServer (DB/SSE hydration)', () => {
  it('REPLACES the local override wholesale (the DB is authoritative)', () => {
    patchComposeSelection('t-1', { model: 'stale', reasoningEffort: 'stale-effort' });
    setComposeSelectionFromServer('t-1', { model: 'fresh', codingAgent: 'codex' });
    // The stale `reasoningEffort` is gone — replace, not merge.
    expect(getComposeSelectionOverride('t-1')).toEqual({ model: 'fresh', codingAgent: 'codex' });
  });

  it('a null/empty payload clears the local entry (no stored selection)', () => {
    patchComposeSelection('t-1', { model: 'opus' });
    setComposeSelectionFromServer('t-1', null);
    currentModel.value = 'g-model';
    expect(resolveModel('t-1')).toBe('g-model'); // back to the account default
    // Empty object is treated the same as null.
    patchComposeSelection('t-2', { model: 'opus' });
    setComposeSelectionFromServer('t-2', {});
    expect(resolveModel('t-2')).toBe('g-model');
  });

  it('hydrating one draft does not touch another', () => {
    patchComposeSelection('a', { model: 'a-model' });
    setComposeSelectionFromServer('b', { model: 'b-model' });
    expect(resolveModel('a')).toBe('a-model');
    expect(resolveModel('b')).toBe('b-model');
  });
});

describe('overrides are per-draft', () => {
  it('an override on one draft does not change another draft', () => {
    currentModel.value = 'default-model';
    patchComposeSelection('t-1', { model: 'opus' });
    expect(resolveModel('t-1')).toBe('opus');
    // t-2 has no override → still the global default.
    expect(resolveModel('t-2')).toBe('default-model');
  });

  it('independent scope + coding agent per draft', () => {
    patchComposeSelection('t-1', { scope: { kind: 'external', repoId: 'repo-a' }, codingAgent: 'codex' });
    patchComposeSelection('t-2', { scope: { kind: 'app', appId: 'app-b' } });
    expect(resolveScope('t-1')).toEqual({ kind: 'external', repoId: 'repo-a' });
    expect(resolveCodingAgent('t-1')).toBe('codex');
    expect(resolveScope('t-2')).toEqual({ kind: 'app', appId: 'app-b' });
    // t-2 never set codingAgent → global default.
    expect(resolveCodingAgent('t-2')).toBe('claude-code');
  });
});

describe('editing a draft never writes the globals', () => {
  it('leaves every global default untouched', () => {
    selectedScope.value = { kind: 'lucidos' };
    selectedCodingAgent.value = 'claude-code';
    currentModel.value = 'g-model';
    reasoningEffort.value = 'g-effort';
    patchComposeSelection('t-1', {
      scope: { kind: 'external', repoId: 'repo-a' },
      codingAgent: 'codex',
      model: 'opus',
      reasoningEffort: 'high',
      ccModel: 'sonnet',
      ccReasoningEffort: 'high',
    });
    expect(selectedScope.value).toEqual({ kind: 'lucidos' });
    expect(selectedCodingAgent.value).toBe('claude-code');
    expect(currentModel.value).toBe('g-model');
    expect(reasoningEffort.value).toBe('g-effort');
    expect(codingAgentPendingModel.value).toBeNull();
    expect(codingAgentPendingReasoningEffort.value).toBeNull();
  });
});

describe('resolveCc* is draft-only (never inherits the active-thread global pending)', () => {
  it('no draft override → null (no pick), even when the global pending is set', () => {
    // The global pending is the ACTIVE-thread mechanism; a fresh compose draft
    // must not inherit it. null → the send omits the field → backend default.
    codingAgentPendingModel.value = 'haiku';
    expect(resolveCcModel('t-1')).toBeNull();
    expect(resolveCcReasoningEffort('t-1')).toBeNull();
  });

  it('explicit null override → the default pick (null)', () => {
    codingAgentPendingModel.value = 'haiku';
    patchComposeSelection('t-1', { ccModel: null });
    expect(resolveCcModel('t-1')).toBeNull();
  });

  it('an explicit draft override is what resolves', () => {
    codingAgentPendingModel.value = 'haiku';
    patchComposeSelection('t-1', { ccModel: 'sonnet', ccReasoningEffort: 'low' });
    expect(resolveCcModel('t-1')).toBe('sonnet');
    expect(resolveCcReasoningEffort('t-1')).toBe('low');
  });
});

describe('the pending slot (no focused draft) — the leak the bug report is about', () => {
  it('a null-threadId patch writes ONLY the pending slot, never a global', () => {
    selectedScope.value = { kind: 'lucidos' };
    selectedCodingAgent.value = 'claude-code';
    currentModel.value = 'g-model';
    patchComposeSelection(null, {
      scope: { kind: 'external', repoId: 'repo-a' },
      codingAgent: 'codex',
      model: 'opus',
    });
    // Pending slot captured it…
    expect(pendingComposeSelection.value).toMatchObject({
      scope: { kind: 'external', repoId: 'repo-a' },
      codingAgent: 'codex',
      model: 'opus',
    });
    // …and NOT a single global (which every override-less draft reads).
    expect(selectedScope.value).toEqual({ kind: 'lucidos' });
    expect(selectedCodingAgent.value).toBe('claude-code');
    expect(currentModel.value).toBe('g-model');
  });

  it('a null-threadId pick does not change what an existing draft resolves', () => {
    selectedCodingAgent.value = 'claude-code';
    // Existing draft has no override → resolves the stable global default.
    expect(resolveCodingAgent('existing')).toBe('claude-code');
    // Fresh-compose pick goes to pending…
    patchComposeSelection(null, { codingAgent: 'codex' });
    // …the existing draft is unaffected (still the global default)…
    expect(resolveCodingAgent('existing')).toBe('claude-code');
    // …while the no-draft compose context resolves the pending pick.
    expect(resolveCodingAgent(null)).toBe('codex');
  });

  it('resolvers with a null threadId read the pending slot (?? global)', () => {
    currentModel.value = 'g-model';
    expect(resolveModel(null)).toBe('g-model'); // empty pending → global
    patchComposeSelection(null, { model: 'opus' });
    expect(resolveModel(null)).toBe('opus');
    expect(getComposeSelectionOverride(null)).toBe(pendingComposeSelection.value);
  });

  it('take + seed transfers the pending pick onto a new draft, then clears pending', () => {
    patchComposeSelection(null, { codingAgent: 'codex', scope: { kind: 'app', appId: 'a' } });
    const taken = takePendingComposeSelection();
    expect(taken).toMatchObject({ codingAgent: 'codex', scope: { kind: 'app', appId: 'a' } });
    // Pending is cleared after taking.
    expect(pendingComposeSelection.value).toEqual({});
    seedComposeSelection('new-draft', taken);
    expect(resolveCodingAgent('new-draft')).toBe('codex');
    expect(resolveScope('new-draft')).toEqual({ kind: 'app', appId: 'a' });
    // A brand-new (unseeded) compose context is back to the default.
    expect(getComposeSelectionOverride(null)).toEqual({});
  });

  it('clearComposeSelection(null) empties the pending slot', () => {
    patchComposeSelection(null, { model: 'opus' });
    clearComposeSelection(null);
    expect(pendingComposeSelection.value).toEqual({});
  });

  it('seedComposeSelection is a no-op for an empty transfer (draft keeps defaults)', () => {
    seedComposeSelection('d', {});
    currentModel.value = 'g-model';
    expect(resolveModel('d')).toBe('g-model');
  });
});

describe('clearComposeSelection', () => {
  it('drops the override so resolves return the current default again', () => {
    currentModel.value = 'default-model';
    patchComposeSelection('t-1', { model: 'opus' });
    expect(resolveModel('t-1')).toBe('opus');
    clearComposeSelection('t-1');
    expect(resolveModel('t-1')).toBe('default-model');
  });

  it('a later patch preserves earlier fields', () => {
    patchComposeSelection('t-1', { model: 'opus' });
    patchComposeSelection('t-1', { reasoningEffort: 'high' });
    expect(resolveModel('t-1')).toBe('opus');
    expect(resolveReasoningEffort('t-1')).toBe('high');
  });
});
