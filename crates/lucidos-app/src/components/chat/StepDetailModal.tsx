import { useEffect, useMemo, useState } from 'preact/hooks';
import { stepDetailModal } from '../../store/store';
import { Overlay } from '../shared/Overlay';
import { formatMessageTimestamp } from '../../utils/formatTime';
import { stepStatus } from '../../store/thread-events';
import type { ContextSection, ContextCapture, Loadable } from '../../store/types';
import { toFailed } from '../../store/types';
import { formatTokens, estimateTokens, contextPercent } from '../../utils/formatTokens';
import { highlightEllipsis } from './highlightEllipsis';
import { groupSections, type RoleGroup, type InnerGroup } from './contextGrouping';
import { fetchContextCapture, fetchToolResult, type ContextCapturePayload } from '../../api/threads';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { mergeContextCaptureSections, needsLazyFetch } from './loadStrippedSections';

function close() {
  stepDetailModal.value = null;
}

function formatChars(n: number): string {
  if (n >= 1000) return `${(n / 1000).toFixed(1)}K`;
  return `${n}`;
}

function ContextSectionRow({ section }: { section: ContextSection }) {
  const [open, setOpen] = useState(false);
  return (
    <div class="context-section" data-role="section-row">
      <button class="context-header context-section-header" onClick={() => setOpen(!open)}>
        <span>{open ? '▼' : '▶'}</span>
        <span class="context-section-name">{section.name}</span>
        <span class="context-section-chars">
          {formatChars(section.char_count)} chars · ≈{formatTokens(estimateTokens(section.char_count))} tokens
        </span>
      </button>
      {open && section.content !== undefined && (
        <pre class="context-section-content">{section.content}</pre>
      )}
      {open && section.content === undefined && (
        <div class="context-section-content empty">
          Body not captured for this section.
        </div>
      )}
    </div>
  );
}

function ContextInnerGroup({ group }: { group: InnerGroup }) {
  const [open, setOpen] = useState(true);
  const totalChars = group.sections.reduce((a, s) => a + s.char_count, 0);
  return (
    <div class="context-inner-group">
      <button class="context-inner-header" onClick={() => setOpen(!open)}>
        <span>{open ? '▾' : '▸'}</span>
        <span class="context-inner-label">{group.name}</span>
        <span class="context-inner-chars">
          {formatChars(totalChars)} · ≈{formatTokens(estimateTokens(totalChars))}
        </span>
      </button>
      {open && group.sections.map(s => <ContextSectionRow key={s.name} section={s} />)}
    </div>
  );
}

function ContextRoleGroup({ role }: { role: RoleGroup }) {
  const [open, setOpen] = useState(true);
  const totalChars = role.innerGroups
    .flatMap(ig => ig.sections)
    .reduce((a, s) => a + s.char_count, 0);
  return (
    <div class="context-role">
      <button class="context-role-header" onClick={() => setOpen(!open)}>
        <span>{open ? '▼' : '▶'}</span>
        <span class="context-role-label">{role.label}</span>
        <span class="context-role-chars">
          {formatChars(totalChars)} chars · ≈{formatTokens(estimateTokens(totalChars))} tokens
        </span>
      </button>
      {open && role.innerGroups.map(ig => (
        ig.name
          ? <ContextInnerGroup key={ig.name} group={ig} />
          : ig.sections.map(s => <ContextSectionRow key={s.name} section={s} />)
      ))}
    </div>
  );
}

/** Sections + tools area. Lazy-fetches when the server stripped them on the
 *  snapshot endpoint (see `api/threads.rs :: strip_context_capture_sections`).
 *  Renders loading/error/loaded states from a `Loadable<ContextCapturePayload>`
 *  per `.claude/rules/frontend.md`'s "Async Data Loading" rule. The
 *  surrounding inline-chip fields (tokens, model, usage) render synchronously
 *  from `snap` either way; only the sections + tools detail block needs the
 *  async hydration. */
function ContextSectionsArea({ snap }: { snap: ContextCapture }) {
  // Synchronous inline-rendered captures (live SSE, legacy synth, or any
  // path where the server didn't strip) resolve to a `loaded` Loadable with
  // the existing fields, so the render path below is uniform.
  const inlineLoadable: Loadable<ContextCapturePayload> = useMemo(() => ({
    status: 'loaded',
    data: { sections: snap.sections, tools: snap.tools },
  }), [snap]);
  const [loadable, setLoadable] = useState<Loadable<ContextCapturePayload>>(
    needsLazyFetch(snap) ? { status: 'loading' } : inlineLoadable,
  );

  // Deps include `inlineLoadable` (a useMemo keyed on `snap`, already in deps):
  // its identity is stable across the in-flight fetch, and the `cancelled` flag
  // guards the async write so a late resolve can't clobber a newer snapshot.
  // Keep both invariants if you ever refactor this dependency list.
  useEffect(() => {
    if (!needsLazyFetch(snap)) {
      setLoadable(inlineLoadable);
      return;
    }
    const eventId = snap.event_id;
    if (!eventId) {
      // Stripped marker without an event id is an upstream contract break.
      // Toast is wrong here — the inline `failed` Loadable below already
      // surfaces the issue in the open modal where the user is looking,
      // and the next open of the same step would re-trigger the same fetch
      // attempt (self-recovering once the upstream fix lands).
      console.warn('[StepDetailModal] ContextCapture is sections_stripped but has no event_id; cannot lazy-fetch.');
      setLoadable(toFailed<ContextCapturePayload>(new Error('missing event id')));
      return;
    }
    let cancelled = false;
    setLoadable({ status: 'loading' });
    fetchContextCapture(eventId)
      .then(payload => {
        if (cancelled) return;
        setLoadable({ status: 'loaded', data: payload });
      })
      .catch((err: unknown) => {
        if (cancelled) return;
        setLoadable(toFailed<ContextCapturePayload>(err));
      });
    return () => { cancelled = true; };
  }, [snap, inlineLoadable]);

  const showLoading = useDelayedLoading(loadable);
  if (loadable.status === 'failed') {
    return <div class="context-sections-error" data-role="context-sections-error">Failed to load sections: {loadable.error}</div>;
  }
  if (loadable.status !== 'loaded') {
    if (!showLoading) return null;
    return <div class="context-sections-loading" data-role="context-sections-loading">Loading sections…</div>;
  }
  const hydrated = mergeContextCaptureSections(snap, loadable.data);
  return (
    <>
      <div class="step-detail-context-meta">
        <code>{hydrated.model || '(unknown model)'}</code>
        <span> · {hydrated.producer === 'claude_code' ? 'Claude Code' : hydrated.producer === 'codex' ? 'Codex' : 'Main LLM'}</span>
        <span> · {hydrated.tools.length} tools</span>
        {hydrated.legacy && <span class="context-legacy-badge" data-tooltip="Synthesized from legacy events">legacy capture</span>}
      </div>
      <div class="context-sections">
        {groupSections(hydrated.sections).map(role => (
          <ContextRoleGroup key={role.role} role={role} />
        ))}
      </div>
    </>
  );
}

/** ToolResult.result area. Renders inline if the snapshot already carried it
 *  (live SSE, `?include_context=true`, or any path where the server didn't
 *  strip). Otherwise lazy-fetches via `GET /events/:event_id/tool-result` —
 *  mirrors `ContextSectionsArea` above, including the
 *  `Loadable<T>` shape per `.claude/rules/frontend.md`'s "Async Data Loading"
 *  rule. `null` result is the image-only ToolResult contract: the
 *  surrounding `<pre>` block is elided. */
function ResultArea({
  inlineResult,
  resultStripped,
  resultEventId,
}: {
  inlineResult: string | undefined;
  resultStripped: boolean | undefined;
  resultEventId: string | undefined;
}) {
  const inlineLoadable: Loadable<{ result: string | null }> = useMemo(() => ({
    status: 'loaded',
    data: { result: inlineResult ?? null },
  }), [inlineResult]);
  const [loadable, setLoadable] = useState<Loadable<{ result: string | null }>>(
    resultStripped ? { status: 'loading' } : inlineLoadable,
  );

  // Deps include `inlineLoadable` (a useMemo keyed on `inlineResult`): its
  // identity is stable across the in-flight fetch, and the `cancelled` flag
  // guards the async write so a late resolve can't clobber a newer result.
  // Keep both invariants if you ever refactor this dependency list.
  useEffect(() => {
    if (!resultStripped) {
      setLoadable(inlineLoadable);
      return;
    }
    if (!resultEventId) {
      // Stripped marker without an event id is an upstream contract break —
      // mirror ContextSectionsArea above. Toast is wrong (the modal already
      // surfaces the failure where the user is looking); console.warn flags
      // it for developer console + failed Loadable shows "Failed to load
      // result" inline. Next re-open of the same step re-runs the fetch
      // attempt (self-recovering once the upstream fix lands).
      console.warn('[StepDetailModal] ToolResult is result_stripped but has no event_id; cannot lazy-fetch.');
      setLoadable(toFailed<{ result: string | null }>(new Error('missing event id')));
      return;
    }
    let cancelled = false;
    setLoadable({ status: 'loading' });
    fetchToolResult(resultEventId)
      .then(payload => {
        if (cancelled) return;
        setLoadable({ status: 'loaded', data: payload });
      })
      .catch((err: unknown) => {
        if (cancelled) return;
        setLoadable(toFailed<{ result: string | null }>(err));
      });
    return () => { cancelled = true; };
  }, [resultStripped, resultEventId, inlineLoadable]);

  const showLoading = useDelayedLoading(loadable);
  if (loadable.status === 'failed') {
    return (
      <>
        <div class="step-detail-section-label">Result</div>
        <div class="step-detail-result-error" data-role="result-error">Failed to load result: {loadable.error}</div>
      </>
    );
  }
  if (loadable.status !== 'loaded') {
    if (!showLoading) return null;
    return (
      <>
        <div class="step-detail-section-label">Result</div>
        <div class="step-detail-result-loading" data-role="result-loading">Loading result…</div>
      </>
    );
  }
  const text = loadable.data.result;
  if (!text) return null; // image-only or genuinely empty — match prior `{step.result && …}` semantics
  return (
    <>
      <div class="step-detail-section-label">Result</div>
      <pre class="step-detail-result">{text}</pre>
    </>
  );
}

/** Budget bar + section list + (when usage is present) cache breakdown. */
function ContextCapturePanel({ snap }: { snap: ContextCapture }) {
  const used = snap.usage?.input_tokens ?? snap.estimated_total_tokens;
  const pct = contextPercent(used, snap.context_window);
  const cacheRead = snap.usage?.cache_read_tokens ?? 0;
  const cacheWrite = snap.usage?.cache_creation_tokens ?? 0;
  // Real input minus what was cached — what the user paid full price for.
  const cacheMiss = snap.usage
    ? Math.max(0, snap.usage.input_tokens - cacheRead - cacheWrite)
    : 0;
  return (
    <div class="step-detail-context">
      <div class="context-budget" data-role="budget-bar">
        <div class="context-budget-row">
          <span>
            <strong>{formatTokens(used)}</strong> / {formatTokens(snap.context_window)} tokens
            {' '}({pct}%){snap.usage ? '' : ' (est.)'}
          </span>
          {snap.trimmed && <span class="context-budget-trimmed">trimmed</span>}
        </div>
        <div class="progress-bar">
          <div class="progress-bar-fill" style={`width: ${pct}%`} />
        </div>
      </div>
      <ContextSectionsArea snap={snap} />
      {snap.usage && (
        <div class="context-usage" data-role="usage-row">
          <span>input <strong>{formatTokens(snap.usage.input_tokens)}</strong></span>
          <span>output <strong>{formatTokens(snap.usage.output_tokens)}</strong></span>
          <span>cache: read <strong>{formatTokens(cacheRead)}</strong> · write <strong>{formatTokens(cacheWrite)}</strong> · miss <strong>{formatTokens(cacheMiss)}</strong></span>
        </div>
      )}
    </div>
  );
}

export function StepDetailModal() {
  const step = stepDetailModal.value;
  if (!step) return null;

  const status = stepStatus(step.success);
  const showFull = step.full && step.full !== step.description;
  const snap = step.contextCapture;

  return (
    <Overlay
      open
      onClose={close}
      overlayClass="step-detail-overlay"
      panelClass="step-detail-modal"
      panelRole="dialog"
      ariaModal
      dataRole="context-captured-modal"
    >
        <div class="step-detail-header">
          <span class={`step-detail-status ${status.className}`}>{status.label}</span>
          {step.created && (
            <span class="step-detail-timestamp">{formatMessageTimestamp(step.created)}</span>
          )}
        </div>
        <div class="step-detail-description">{highlightEllipsis(step.description)}</div>
        {step.detail && <div class="step-detail-detail">{highlightEllipsis(step.detail)}</div>}
        {showFull && <pre class="step-detail-full">{step.full}</pre>}
        <ResultArea
          inlineResult={step.result}
          resultStripped={step.result_stripped}
          resultEventId={step.result_event_id}
        />
        {snap
          ? <ContextCapturePanel snap={snap} />
          : <div class="step-detail-empty">No context snapshot captured for this step.</div>}
        <button class="action-btn step-detail-close" onClick={close}>Close</button>
    </Overlay>
  );
}
