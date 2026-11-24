import { useEffect, useMemo } from 'preact/hooks';
import { stepDetailModal } from '../../store/store';
import { Overlay } from '../shared/Overlay';
import { SurfaceHead } from '../shared/Surface';
import { formatMessageTimestamp } from '../../utils/formatTime';
import { stepStatus } from '../../store/thread-events';
import type { Loadable, StepOutcome } from '../../store/types';
import { toFailed } from '../../store/types';
import { highlightEllipsis } from './highlightEllipsis';
import { StepRecallList } from './StepRecallList';
import { requestToolArgs, requestToolResult } from '../../store/stepDetailCache';
import { fullCommandForCCTool, fullCommandForEngineTool } from '../../store/thread-events/tool-descriptions';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { usePaneCentre } from '../../hooks/usePaneCentre';
import { StepOutcomeIcon } from '../shared/icons';
import { LoadingFade } from '../shared/LoadingFade';
import { SkText, SkeletonProvider, useSkeleton } from '../shared/Skeleton';

const STEP_WITHOUT_ID = 'the step was saved without an id, so it cannot be looked up';

function close() {
  stepDetailModal.value = null;
}

const CODE_SKELETON_LINES = ['92%', '78%', '85%', '40%'];

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

function ResultArea({
  inlineResult,
  resultStripped,
  resultEventId,
}: {
  inlineResult: string | undefined;
  resultStripped: boolean | undefined;
  resultEventId: string | undefined;
}) {
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
    content = <ResultBlock text={loadable.data.result} />;
  }
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
  const missingId = !!argsStripped && !callEventId;
  useEffect(() => {
    if (missingId) console.warn('[StepDetailModal] tool call is args_stripped but has no event_id; cannot lazy-fetch.');
  }, [missingId]);
  const fetched = useMemo(
    () => (argsStripped && callEventId ? requestToolArgs(callEventId) : null),
    [argsStripped, callEventId],
  );
  const fetchedArgs = fetched?.value;
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

const STEP_DETAIL_NOTE: Partial<Record<StepOutcome, string>> = {
  unfinished: 'The turn ended before this tool reported a result, so what it did (if anything) was not recorded.',
  blocked: 'Waiting for your decision on the permission card. The tool has not run, so there is nothing to report yet.',
};

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
        {step.recall && <StepRecallList recall={step.recall} />}
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
