import { blobPreviewUrl, continueThread } from '../../api/client';
import type { Change } from '../../api/client';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { ensureChangeLoaded, revertChange } from '../../store/actions/chat-changes';
import { viewChangeDiff } from '../../store/actions/repositories';
import { findChangeById, lazyChanges, openImagePopupFromGroup, showToast, stepDetailModal } from '../../store/store';
import { LUCIDOS_AGENT_ICON, LUCIDOS_AGENT_LABEL, resumeEngineNote, stepStatus } from '../../store/thread-events';
import type { Exchange } from '../../store/thread-events';
import type { Loadable, ResponseEvent } from '../../store/types';
import { errorDetail } from '../../utils/errorDetail';
import { contextPercent, formatTokens } from '../../utils/formatTokens';
import { ClaudeIcon } from '../shared/icons';
import { highlightEllipsis } from './highlightEllipsis';
import { getSessionBlobUrlForHash } from './pastedImages';
import { useSignal } from '@preact/signals';
import type { ComponentChildren } from 'preact';
import { useEffect } from 'preact/hooks';
import type { InitiatorDescriptor } from './ChatExchange';

// Presentational sub-components for ChatExchange (panels, bodies, response
// rendering). Extracted from ChatExchange.tsx; imported back there. The only
// dependency on the parent is the InitiatorDescriptor type (erased at runtime).

const CHANGE_ACCENT = {
  ChangeApplied: 'change-applied',
  ChangeDiscarded: 'change-discarded',
  ChangeReverted: 'change-reverted',
} as const;

export function changeAccent(type: keyof typeof CHANGE_ACCENT): string {
  return CHANGE_ACCENT[type];
}

export function MarkdownBlock({ html }: { html: string }) {
  return <div class="markdown-content" dangerouslySetInnerHTML={{ __html: html }} />;
}

export function FileList({ files }: { files: string[] }) {
  return (
    <ul class="initiator-files">
      {files.map(f => <li key={f}><code>{f}</code></li>)}
    </ul>
  );
}

export function UserMessageBody({ html, imageHashes }: { html: string; imageHashes: string[] }) {
  return (
    <>
      {html && <div class="markdown-content" dangerouslySetInnerHTML={{ __html: html }} />}
      {imageHashes.length > 0 && (
        <div class="user-images">
          {imageHashes.map((hash, i) => {
            const src = getSessionBlobUrlForHash(hash) ?? blobPreviewUrl(hash);
            return (
              <img
                key={hash + ':' + i}
                src={src}
                class="user-image-thumb"
                alt=""
                onClick={(e) => openImagePopupFromGroup(e.currentTarget.src, e.currentTarget)}
              />
            );
          })}
        </div>
      )}
    </>
  );
}

/** Body for change lifecycle initiator panels — surfaces the change description,
 *  file count, and (when failed) the error message. The resolved timestamp is
 *  the panel header timestamp (the change-lifecycle event time IS resolved_at). */
export function ChangeBody({ changeId, error }: { changeId?: string; error?: string }) {
  const change: Change | undefined = changeId ? findChangeById(changeId) : undefined;
  const lazy: Loadable<Change> = (changeId ? lazyChanges.value.get(changeId) : undefined) ?? { status: 'not-loaded' };
  const showLoading = useDelayedLoading(lazy);

  useEffect(() => {
    if (changeId) void ensureChangeLoaded(changeId);
  }, [changeId]);

  // Lifecycle error and live data both win over the lazy-fetch state — a 404
  // for a row that arrives via SSE moments later shouldn't strand a stale
  // "Failed to load" row beneath the now-resolved description.
  const lazyFailedError = !error && !change && lazy.status === 'failed'
    ? `Failed to load change details: ${lazy.error}`
    : undefined;
  const lazyLoading = !change && lazy.status === 'loading' && showLoading;

  const desc = change ? change.description.split('\n')[0] : undefined;
  const fileCount = change?.file_count;

  if (!desc && fileCount == null && !error && !lazyFailedError && !lazyLoading) return null;
  return (
    <div class="change-body">
      {desc && <div class="change-body-desc">{desc}</div>}
      {fileCount != null && (
        <div class="change-body-meta">{fileCount} file{fileCount !== 1 ? 's' : ''}</div>
      )}
      {error && <div class="change-body-error">{error}</div>}
      {lazyFailedError && <div class="change-body-error">{lazyFailedError}</div>}
      {lazyLoading && <div class="change-body-meta">Loading...</div>}
    </div>
  );
}

/** "Continue" button rendered on the most recent unresumed ResponseAborted
 *  exchange. Disables itself between click and response so a double-click
 *  can't double-emit. Surfaces network failures via toast and re-enables. */
export function ContinueButton({ threadId }: { threadId: string }) {
  const inFlight = useSignal(false);
  const onClick = async (e: MouseEvent) => {
    e.stopPropagation();
    if (inFlight.value) return;
    inFlight.value = true;
    try {
      await continueThread(threadId);
    } catch (err) {
      showToast(`Failed to continue: ${errorDetail(err)}`, 'error');
      inFlight.value = false;
      return;
    }
    // ContinuationStarted will arrive via SSE and remove the button by hiding
    // this exchange's `isUnresumedAbort` — re-enable as a safety net in case
    // the SSE event is delayed.
    setTimeout(() => { inFlight.value = false; }, 5000);
  };
  return (
    <button class="action-btn" onClick={onClick} disabled={inFlight.value}>
      {inFlight.value ? 'Continuing...' : 'Continue'}
    </button>
  );
}

/** Render the engine note for a ContinuationStarted exchange — a one-line
 *  subline followed by a `<details>` expansion showing the full injected text.
 *  Returns null when no engine note is present (e.g. CC resume path). */
export function ResumeNoteBody({ exchange }: { exchange: Exchange }) {
  const note = resumeEngineNote(exchange);
  if (!note) return null;
  const subline = note.toolCount > 0
    ? `Reminded the model about ${note.toolCount} prior tool call${note.toolCount === 1 ? '' : 's'}`
    : 'Reminded the model that no actions had completed';
  return (
    <details class="resume-note">
      <summary>{subline}</summary>
      <pre class="resume-note-body">{note.text}</pre>
    </details>
  );
}

/** Diff/Revert action buttons rendered in the initiator panel's action slot
 *  for ChangeApplied/Discarded/Reverted exchanges. Returns null when the
 *  change has no relevant actions (e.g. ChangeApplyFailed leaves the change
 *  pending — user reads the error, doesn't diff/revert). */
export function changeActions(changeId?: string, suppress?: boolean): ComponentChildren {
  if (suppress || !changeId) return null;
  const change = findChangeById(changeId);
  if (!change) return null;
  const showDiff = change.status === 'pending' || !!change.pre_merge_sha;
  const showRevert = change.status === 'applied';
  if (!showDiff && !showRevert) return null;
  return (
    <>
      {showDiff && <button class="action-btn" onClick={() => void viewChangeDiff(change)}>Diff</button>}
      {showRevert && <button class="action-btn action-btn-danger" onClick={() => void revertChange(change.id)}>Revert</button>}
    </>
  );
}

/** Shared header click handler for InitiatorPanel/ResponsePanel — toggles the
 *  panel's collapsed state, but ignores clicks that originated on a button or
 *  link inside the header (actor info, executor info, stop). */
function handlePanelHeaderClick(e: MouseEvent, onToggle?: () => void): void {
  if (!onToggle) return;
  if ((e.target as HTMLElement).closest('button, a')) return;
  onToggle();
}

interface InitiatorPanelProps {
  initiator: InitiatorDescriptor;
  timestamp: string;
  onActorClick?: (e: MouseEvent) => void;
  actions?: ComponentChildren;
  collapsible: boolean;
  collapsed: boolean;
  onToggle?: () => void;
}

function ActorChipBody({ initiator }: { initiator: InitiatorDescriptor }) {
  return (
    <>
      <span class="initiator-icon">{initiator.icon}</span>
      <span class="initiator-label">{initiator.label}</span>
    </>
  );
}

export function InitiatorPanel({ initiator, timestamp, onActorClick, actions, collapsible, collapsed, onToggle }: InitiatorPanelProps) {
  const accentClass = initiator.accent ? ` initiator-panel-${initiator.accent}` : '';
  const hasBody = !!initiator.summary || !!initiator.details;

  return (
    <div class={`initiator-panel initiator-panel-${initiator.variant}${accentClass}${collapsed ? ' initiator-panel-collapsed' : ''}`}>
      <div
        class={`initiator-header${collapsible ? ' initiator-header-clickable' : ''}`}
        onClick={(e) => handlePanelHeaderClick(e, onToggle)}
      >
        {onActorClick ? (
          <button
            type="button"
            class="initiator-actor"
            onClick={onActorClick}
            aria-label={`Show info for ${initiator.label}`}
          >
            <ActorChipBody initiator={initiator} />
          </button>
        ) : (
          <span class="initiator-actor">
            <ActorChipBody initiator={initiator} />
          </span>
        )}
        <span class="initiator-timestamp">{timestamp}</span>
      </div>
      {hasBody && !collapsed && (
        <div class="initiator-body">
          {initiator.summary && <div class="initiator-summary">{initiator.summary}</div>}
          {initiator.details}
        </div>
      )}
      {actions && !collapsed && <div class="initiator-footer">{actions}</div>}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Response panel — bordered card wrapping the executor's reply. Header carries
// executor + status + (optional) stop button + timestamp + collapse toggle.
// Body is the response content; collapses when the user clicks the header.
// ---------------------------------------------------------------------------

/** Triggers invoke the Lucidos agent rather than running their own executor,
 *  so the label always reflects the LLM that produced the response. The model
 *  is shown in the executor info popover, not in the header. The "Lucidos
 *  Agent" name matches the initiator label used when one Lucidos thread
 *  spawns another — same entity, same display. */
export function describeExecutor(
  isCC: boolean,
): { icon: ComponentChildren; label: string } {
  if (isCC) return { icon: <ClaudeIcon />, label: 'Claude Code' };
  return { icon: LUCIDOS_AGENT_ICON, label: LUCIDOS_AGENT_LABEL };
}

interface ResponsePanelProps {
  executor: { icon: ComponentChildren; label: string };
  onExecutorClick?: (e: MouseEvent) => void;
  status: ComponentChildren;
  timestamp: string;
  collapsible: boolean;
  collapsed: boolean;
  onToggle?: () => void;
  hasBody: boolean;
  children: ComponentChildren;
}

export function ResponsePanel({
  executor, onExecutorClick, status, timestamp, collapsible, collapsed, onToggle, hasBody, children,
}: ResponsePanelProps) {
  return (
    <div class={`response-panel${collapsed ? ' response-panel-collapsed' : ''}${hasBody ? '' : ' response-panel-bodyless'}`}>
      <div
        class={`response-header${collapsible ? ' response-header-clickable' : ''}`}
        onClick={(e) => handlePanelHeaderClick(e, onToggle)}
      >
        <button
          type="button"
          class="response-executor"
          onClick={onExecutorClick}
          aria-label="Show executor info"
        >
          <span class="response-executor-icon">{executor.icon}</span>
          <span class="response-executor-label">{executor.label}</span>
        </button>
        <span class="response-meta">
          {status}
          <span class="response-timestamp">{timestamp}</span>
        </span>
      </div>
      {hasBody && !collapsed && (
        <div class="response-body">
          {children}
        </div>
      )}
    </div>
  );
}

/** Generated image rendered inline in the response. */
export function GeneratedImage({ event }: { event: Extract<ResponseEvent, { type: 'image' }> }) {
  const src = `data:${event.mime_type};base64,${event.base64}`;
  return (
    <div class="generated-image" data-tooltip={`thread:${event.index}`}>
      <img
        src={src}
        class="image-thumbnail"
        alt="Generated image"
        loading="lazy"
      />
    </div>
  );
}

/** Prefers `contextCapture` (new emissions + legacy synth); falls back
 *  to the legacy `context_tokens` projection so old DB rows still render. */
export function InlineStep({ event }: { event: Extract<ResponseEvent, { type: 'step' }> }) {
  const { className } = stepStatus(event.success);
  const snap = event.contextCapture;
  const used = snap?.usage?.input_tokens ?? snap?.estimated_total_tokens ?? event.context_tokens;
  const window = snap?.context_window;
  const trimmed = snap?.trimmed ?? event.trimmed;
  const hasContext = used != null;

  return (
    <button
      type="button"
      class={`inline-step ${className}`}
      data-role="inline-step"
      onClick={() => { stepDetailModal.value = event; }}
    >
      <span class="step-icon">
        {event.success === null ? <span class="mini-spinner" /> : event.success ? '✓' : '⚠'}
      </span>
      <span class="step-description">{highlightEllipsis(event.description)}</span>
      {event.detail && <span class="step-detail">{highlightEllipsis(event.detail)}</span>}
      {hasContext && (
        <span class={`step-context${trimmed ? ' trimmed' : ''}`}>
          {window
            ? `${formatTokens(used!)} / ${formatTokens(window)} (${contextPercent(used!, window)}%)`
            : `${formatTokens(used!)} tokens${event.context_messages != null ? `, ${event.context_messages} msgs` : ''}`}
          {trimmed && ' · trimmed'}
        </span>
      )}
    </button>
  );
}
