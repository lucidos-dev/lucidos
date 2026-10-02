import { useEffect, useMemo } from 'preact/hooks';
import { stepDetailModal } from '../../store/store';
import { Overlay } from '../shared/Overlay';
import { SurfaceHead } from '../shared/Surface';
import { formatMessageTimestamp } from '../../utils/formatTime';
import { stepStatus } from '../../store/thread-events';
import type { Loadable, StepOutcome } from '../../store/types';
import { toFailed } from '../../store/types';
import { highlightEllipsis } from './highlightEllipsis';
import { requestToolArgs, requestToolResult } from '../../store/stepDetailCache';
import { fullCommandForCCTool, fullCommandForEngineTool } from '../../store/thread-events/exchange';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { usePaneCentre } from '../../hooks/usePaneCentre';
import { StepOutcomeIcon } from '../shared/icons';
import { LoadingFade } from '../shared/LoadingFade';
import { SkText, SkeletonProvider, useSkeleton } from '../shared/Skeleton';

/** Why a step's detail cannot be fetched: it was recorded with no event id. */
const STEP_WITHOUT_ID = 'the step was saved without an id, so it cannot be looked up';

function close() {
  stepDetailModal.value = null;
}

/** Line widths a code block shimmers with while its text loads. */
const CODE_SKELETON_LINES = ['92%', '78%', '85%', '40%'];

/** A command or result block. Under a `SkeletonProvider` it draws its own box
 *  with shimmer lines, so the placeholder has the loaded block's shape. */
function StepCodeBlock({ class: cls, text }: { class: string; text?: string }) {
  if (useSkeleton()) {
    return (
      <pre class={cls} aria-hidden="true">
        {CODE_SKELETON_LINES.map((w) => <SkText key={w} as="div" w={w} />)}
      </pre>
    );
  }
  return <pre class={cls}>{text}</pre>;
}

function ResultBlock({ text }: { text?: string }) {
  return (
    <>
      <div class="step-detail-section-label">Result</div>
      <StepCodeBlock class="step-detail-result" text={text} />
    </>
  );
}

/** ToolResult.result area. Renders inline if the snapshot already carried it
 *  (live SSE, `?include_context=true`, or any path where the server didn't
 *  strip). Otherwise it reads the step detail cache, which the row usually
 *  filled on press. `null` result is the image-only ToolResult contract: the
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
  // A stripped marker without an event id is an upstream contract break. The
  // failed Loadable says so where the user is looking, and the warning flags
  // it for the developer console. Reopening the step retries.
  const missingId = !!resultStripped && !resultEventId;
  useEffect(() => {
    if (missingId) console.warn('[StepDetailModal] ToolResult is result_stripped but has no event_id; cannot lazy-fetch.');
  }, [missingId]);
  const fetched = useMemo(
    () => (resultStripped && resultEventId ? requestToolResult(resultEventId) : null),
    [resultStripped, resultEventId],
  );
  const loadable: Loadable<{ result: string | null }> = fetched
    ? fetched.value
    : missingId
      ? toFailed<{ result: string | null }>(new Error(STEP_WITHOUT_ID))
      : { status: 'loaded', data: { result: inlineResult ?? null } };

  const showLoading = useDelayedLoading(loadable);
  let content = null;
  if (loadable.status === 'failed') {
    content = (
      <>
        <div class="step-detail-section-label">Result</div>
        <div class="step-detail-result-error" data-role="result-error">Could not load this step’s result: {loadable.error}</div>
      </>
    );
  } else if (loadable.status === 'loaded' && loadable.data.result) {
    // An image-only or empty result draws nothing.
    content = <ResultBlock text={loadable.data.result} />;
  }
  // A settled empty result drops its wrapper too, or the body's gap opens round it.
  if (!resultStripped || (loadable.status === 'loaded' && !content)) return content;
  return (
    <LoadingFade
      class="step-detail-fade"
      showSkeleton={showLoading}
      skeleton={<SkeletonProvider><ResultBlock /></SkeletonProvider>}
    >
      {content}
    </LoadingFade>
  );
}

/** The step's command at the top of the body: its un-elided primary argument,
 *  or the description when that is all there is.
 *
 *  Two sources, one look. `inlineFull` is what the fold computed when the row
 *  still carried its args, which is every live SSE emission. A snapshot row has
 *  had them stripped, so this reads them from the step detail cache. It then
 *  runs the SAME formatter the fold would have, rather than asking the server
 *  for a rendered string. One formatter per channel, so the paths cannot drift.
 *
 *  The full command is the complete form of the step's description. The
 *  description stands in only when there is none, or it adds nothing. */
function CommandArea({
  inlineFull,
  description,
  toolName,
  argsStripped,
  callEventId,
  toolChannel,
}: {
  inlineFull: string | undefined;
  description: string;
  toolName: string | undefined;
  argsStripped: boolean | undefined;
  callEventId: string | undefined;
  toolChannel: 'chat' | 'coding_agent' | undefined;
}) {
  // Same contract break, and same handling, as `ResultArea`'s missing id.
  const missingId = !!argsStripped && !callEventId;
  useEffect(() => {
    if (missingId) console.warn('[StepDetailModal] tool call is args_stripped but has no event_id; cannot lazy-fetch.');
  }, [missingId]);
  const fetched = useMemo(
    () => (argsStripped && callEventId ? requestToolArgs(callEventId) : null),
    [argsStripped, callEventId],
  );
  const fetchedArgs = fetched?.value;
  // The channel decides the formatter, because each has its own and the inline
  // label was built with that one. See `tool_channel`.
  const loadable: Loadable<{ full: string | undefined }> = useMemo(() => {
    if (missingId) return toFailed<{ full: string | undefined }>(new Error(STEP_WITHOUT_ID));
    if (!fetchedArgs) return { status: 'loaded', data: { full: inlineFull } };
    if (fetchedArgs.status !== 'loaded') return fetchedArgs;
    const format = toolChannel === 'chat' ? fullCommandForEngineTool : fullCommandForCCTool;
    return { status: 'loaded', data: { full: format(toolName ?? '', fetchedArgs.data.args) } };
  }, [missingId, fetchedArgs, inlineFull, toolChannel, toolName]);

  const showLoading = useDelayedLoading(loadable);
  const descriptionLine = <div class="step-detail-description">{highlightEllipsis(description)}</div>;
  let content = null;
  if (loadable.status === 'failed') {
    content = (
      <>
        {descriptionLine}
        <div class="step-detail-result-error" data-role="command-error">Could not load this step’s command: {loadable.error}</div>
      </>
    );
  } else if (loadable.status === 'loaded') {
    const full = loadable.data.full;
    content = full && full !== description ? <StepCodeBlock class="step-detail-full" text={full} /> : descriptionLine;
  }
  // Nothing draws before the gate: the description would flash, then give way
  // to the full command on most loads.
  if (!argsStripped) return content;
  return (
    <LoadingFade
      class="step-detail-fade"
      showSkeleton={showLoading}
      skeleton={<SkeletonProvider><StepCodeBlock class="step-detail-full" /></SkeletonProvider>}
    >
      {content}
    </LoadingFade>
  );
}

/** The one-line explanation under the step, for the two outcomes whose
 *  status does not account for an EMPTY result area below it. Without one
 *  the emptiness reads as a second mystery on top of the first.
 *
 *  The rest need no entry. `'success'` and `'error'` have a result, and so does
 *  `'denied'`: the refusal the agent was handed. `'pending'` is self-evident,
 *  the row it was opened from being the one that shimmers. */
const STEP_DETAIL_NOTE: Partial<Record<StepOutcome, string>> = {
  unfinished: 'The turn ended before this tool reported a result, so what it did (if anything) was not recorded.',
  blocked: 'Waiting for your decision on the permission card. The tool has not run, so there is nothing to report yet.',
};

/** The head's tone icon: the transcript row's own outcome mark, or the spinner
 *  of work in flight. The title beside it names the outcome. */
function StepDetailIcon({ outcome }: { outcome: StepOutcome }) {
  return (
    <span
      class={`surface-icon step-detail-icon ${stepStatus(outcome).className}`}
      aria-hidden="true"
      data-role="step-detail-icon"
    >
      {outcome === 'pending' ? <span class="mini-spinner" /> : <StepOutcomeIcon outcome={outcome} />}
    </span>
  );
}

/** What one step DID: its description, the untruncated command behind it, the
 *  reasoning that produced it, and whatever it reported back.
 *
 *  The head stays one short line on any width: the outcome and when. The body
 *  opens on the step itself, as its full command or else its description.
 *
 *  Deliberately NOT the context the model was looking at. That is the *context
 *  viewer*, opened from the step row's context counter, and duplicating it here
 *  would make the counter a second door to the same room. See
 *  `ContextViewerModal`. */
export function StepDetailModal() {
  const step = stepDetailModal.value;
  const paneCentre = usePaneCentre('conversation');
  if (!step) return null;

  return (
    <Overlay
      open
      onClose={close}
      overlayClass="step-detail-overlay"
      panelClass="surface surface-raised surface-pane-centred step-detail-modal"
      panelStyle={paneCentre}
      panelRole="dialog"
      ariaModal
      dataRole="step-detail-modal"
    >
      <SurfaceHead
        icon={<StepDetailIcon outcome={step.outcome} />}
        title={stepStatus(step.outcome).label}
        meta={step.created ? formatMessageTimestamp(step.created) : undefined}
        onClose={close}
        closeLabel="Close step details"
      />
      <div class="surface-body step-detail-body" tabIndex={-1}>
        <CommandArea
          inlineFull={step.full}
          description={step.description}
          toolName={step.tool_name}
          argsStripped={step.args_stripped}
          callEventId={step.call_event_id}
          toolChannel={step.tool_channel}
        />
        {step.detail && <div class="step-detail-detail">{highlightEllipsis(step.detail)}</div>}
        {STEP_DETAIL_NOTE[step.outcome] && (
          <div class="step-detail-note">{STEP_DETAIL_NOTE[step.outcome]}</div>
        )}
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
      </div>
    </Overlay>
  );
}
