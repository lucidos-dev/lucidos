import { focusThread } from '../../store/actions/threads';
import { renderMarkdown } from '../../utils/renderMarkdown';
import type { ChildCompletionStatus } from '../../store/thread-events';

interface Props {
  childThreadId: string;
  childThreadTitle?: string;
  status: ChildCompletionStatus;
  summary: string;
}

const STATUS_CLASS: Record<ChildCompletionStatus, string> = {
  success: 'child-completion-status-success',
  failure: 'child-completion-status-failure',
  no_changes: 'child-completion-status-no-changes',
  canceled: 'child-completion-status-canceled',
};

const STATUS_GLYPH: Record<ChildCompletionStatus, string> = {
  success: '✓',
  failure: '✕',
  no_changes: '○',
  canceled: '⊘',
};

const STATUS_LABEL: Record<ChildCompletionStatus, string> = {
  success: 'success',
  failure: 'failure',
  no_changes: 'no changes',
  canceled: 'canceled',
};

const STATUS_PREFIX: Record<ChildCompletionStatus, string> = {
  success: 'Child thread completed:',
  failure: 'Child thread failed:',
  no_changes: 'Child thread completed:',
  canceled: 'Child thread canceled:',
};

export function ChildCompletionCard(props: Props) {
  const titleText = props.childThreadTitle?.trim() || 'Untitled thread';
  const summaryHtml = props.summary.trim() ? renderMarkdown(props.summary) : '';
  return (
    <div class="child-completion-body">
      <div class="child-completion-header-row">
        <span class="child-completion-prefix">{STATUS_PREFIX[props.status]}</span>
        <button
          type="button"
          class="accent-link"
          onClick={() => focusThread(props.childThreadId)}
          data-thread-id={props.childThreadId}
        >
          {titleText}
        </button>
        <span class={`child-completion-status ${STATUS_CLASS[props.status]}`}>
          {STATUS_GLYPH[props.status]} {STATUS_LABEL[props.status]}
        </span>
      </div>
      {summaryHtml && (
        <details class="child-completion-disclosure">
          <summary>Show summary</summary>
          <div
            class="child-completion-summary markdown-content"
            dangerouslySetInnerHTML={{ __html: summaryHtml }}
          />
        </details>
      )}
    </div>
  );
}
