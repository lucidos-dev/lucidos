import { useEffect, useMemo, useState } from 'preact/hooks';
import { stepDetailModal } from '../../store/store';
import { Overlay } from '../shared/Overlay';
import { formatMessageTimestamp } from '../../utils/formatTime';
import { stepStatus } from '../../store/thread-events';
import type { Loadable, StepOutcome } from '../../store/types';
import { toFailed } from '../../store/types';
import { highlightEllipsis } from './highlightEllipsis';
import { fetchToolArgs, fetchToolResult } from '../../api/threads';
import { fullCommandForCCTool } from '../../store/thread-events/exchange';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';

function close() {
  stepDetailModal.value = null;
}

/** ToolResult.result area. Renders inline if the snapshot already carried it
 *  (live SSE, `?include_context=true`, or any path where the server didn't
 *  strip). Otherwise lazy-fetches via `GET /events/:event_id/tool-result` —
 *  mirrors `ContextSectionsArea` in `ContextCapturePanel`, including the
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
      // mirror ContextSectionsArea. Toast is wrong (the modal already
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

/** The un-elided primary argument, rendered above Reasoning.
 *
 *  Two sources, one look. `inlineFull` is what the fold computed when the row
 *  still carried its args, which is every live SSE emission. A snapshot row has
 *  had them stripped, so this fetches them. It then runs the SAME
 *  `fullCommandForCCTool` the fold would have, rather than asking the server
 *  for a rendered string. One formatter, so the two paths cannot drift.
 *
 *  Elided when it only repeats the description, which is the rule the inline
 *  path has always applied. */
function FullCommandArea({
  inlineFull,
  description,
  toolName,
  argsStripped,
  callEventId,
}: {
  inlineFull: string | undefined;
  description: string;
  toolName: string | undefined;
  argsStripped: boolean | undefined;
  callEventId: string | undefined;
}) {
  const inlineLoadable: Loadable<{ full: string | undefined }> = useMemo(() => ({
    status: 'loaded',
    data: { full: inlineFull },
  }), [inlineFull]);
  const [loadable, setLoadable] = useState<Loadable<{ full: string | undefined }>>(
    argsStripped ? { status: 'loading' } : inlineLoadable,
  );

  // Same shape as `ResultArea`'s effect, including the `cancelled` guard and
  // the stable `inlineLoadable` identity. Keep both if you refactor the deps.
  useEffect(() => {
    if (!argsStripped) {
      setLoadable(inlineLoadable);
      return;
    }
    if (!callEventId) {
      // Stripped marker with no event id is an upstream contract break. Mirrors
      // ResultArea: warn for the developer console, and let the failed Loadable
      // say so where the user is looking. Re-opening the step retries.
      console.warn('[StepDetailModal] tool call is args_stripped but has no event_id; cannot lazy-fetch.');
      setLoadable(toFailed<{ full: string | undefined }>(new Error('missing event id')));
      return;
    }
    let cancelled = false;
    setLoadable({ status: 'loading' });
    fetchToolArgs(callEventId)
      .then(payload => {
        if (cancelled) return;
        setLoadable({
          status: 'loaded',
          data: { full: fullCommandForCCTool(toolName ?? '', payload.args) },
        });
      })
      .catch((err: unknown) => {
        if (cancelled) return;
        setLoadable(toFailed<{ full: string | undefined }>(err));
      });
    return () => { cancelled = true; };
  }, [argsStripped, callEventId, toolName, inlineLoadable]);

  const showLoading = useDelayedLoading(loadable);
  if (loadable.status === 'failed') {
    return <div class="step-detail-result-error" data-role="command-error">Failed to load command: {loadable.error}</div>;
  }
  if (loadable.status !== 'loaded') {
    if (!showLoading) return null;
    return <div class="step-detail-result-loading" data-role="command-loading">Loading command…</div>;
  }
  const full = loadable.data.full;
  if (!full || full === description) return null;
  return <pre class="step-detail-full">{full}</pre>;
}

/** The one-line explanation under the description, for the two outcomes whose
 *  status word does not account for an EMPTY result area below it. Without one
 *  the emptiness reads as a second mystery on top of the first.
 *
 *  The rest need no entry. `'success'` and `'error'` have a result, and so does
 *  `'denied'`: the refusal the agent was handed. `'pending'` is self-evident,
 *  the row it was opened from being the one that shimmers. */
const STEP_DETAIL_NOTE: Partial<Record<StepOutcome, string>> = {
  unfinished: 'The turn ended before this tool reported a result, so what it did (if anything) was not recorded.',
  blocked: 'Waiting for your decision on the permission card. The tool has not run, so there is nothing to report yet.',
};

/** What one step DID: its description, the untruncated command behind it, the
 *  reasoning that produced it, and whatever it reported back.
 *
 *  Deliberately NOT the context the model was looking at. That is the *context
 *  viewer*, opened from the step row's context counter, and duplicating it here
 *  would make the counter a second door to the same room. See
 *  `ContextViewerModal`. */
export function StepDetailModal() {
  const step = stepDetailModal.value;
  if (!step) return null;

  const status = stepStatus(step.outcome);

  return (
    <Overlay
      open
      onClose={close}
      overlayClass="step-detail-overlay"
      panelClass="step-detail-modal"
      panelRole="dialog"
      ariaModal
      dataRole="step-detail-modal"
    >
        <div class="step-detail-header">
          <span class={`step-detail-status ${status.className}`}>{status.label}</span>
          {step.created && (
            <span class="step-detail-timestamp">{formatMessageTimestamp(step.created)}</span>
          )}
        </div>
        <div class="step-detail-description">{highlightEllipsis(step.description)}</div>
        {STEP_DETAIL_NOTE[step.outcome] && (
          <div class="step-detail-note">{STEP_DETAIL_NOTE[step.outcome]}</div>
        )}
        {step.detail && <div class="step-detail-detail">{highlightEllipsis(step.detail)}</div>}
        <FullCommandArea
          inlineFull={step.full}
          description={step.description}
          toolName={step.tool_name}
          argsStripped={step.args_stripped}
          callEventId={step.call_event_id}
        />
        {step.thinkingText && (
          <>
            <div class="step-detail-section-label">Reasoning</div>
            <pre class="step-detail-reasoning">{step.thinkingText}</pre>
          </>
        )}
        <ResultArea
          inlineResult={step.result}
          resultStripped={step.result_stripped}
          resultEventId={step.result_event_id}
        />
        <button class="action-btn step-detail-close" onClick={close}>Close</button>
    </Overlay>
  );
}
