import { blobPreviewUrl, continueThread, postCommandCheckpointUndo } from '../../api/client';
import type { Change } from '../../api/client';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { ensureChangeLoaded, revertChange } from '../../store/actions/chat-changes';
import { viewChangeDiff } from '../../store/actions/repositories';
import { findChangeById, lazyChanges, openImagePopupFromGroup, showToast, stepDetailModal } from '../../store/store';
import { LUCIDOS_AGENT_LABEL, resumeEngineNote, stepStatus } from '../../store/thread-events';
import { LucidosGlyph } from '../shared/LucidosMark';
import { BlobImage } from '../shared/BlobImage';
import type { Exchange } from '../../store/thread-events';
import type { Loadable, ResponseEvent } from '../../store/types';
import type { CodingAgent } from '../../api/types';
import { errorDetail } from '../../utils/errorDetail';
import { formatFileCount } from '../../utils/formatFileCount';
import { contextPercent, formatTokens } from '../../utils/formatTokens';
import { ClaudeIcon, CodexIcon } from '../shared/icons';
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
              <BlobImage
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
 *  the panel header timestamp (the change-lifecycle event time IS resolved_at).
 *
 *  `seedDescription`/`seedFileCount` come from the in-thread `ChangeProposed`
 *  event (already loaded with the thread). They render the body at its FINAL
 *  height on first open — before the per-id `Change` lazy-fetch lands — so a
 *  thread that ends with "Change applied" doesn't jump when the fetched row
 *  pops the description + file count in (the open-path scroll-to-bottom would
 *  re-pin to the grown bottom, shifting everything up). The authoritative live
 *  row wins once loaded; it's normally identical to the seed. */
export function ChangeBody({ changeId, error, seedDescription, seedFileCount }: { changeId?: string; error?: string; seedDescription?: string; seedFileCount?: number }) {
  const change: Change | undefined = changeId ? findChangeById(changeId) : undefined;
  const lazy: Loadable<Change> = (changeId ? lazyChanges.value.get(changeId) : undefined) ?? { status: 'not-loaded' };
  const showLoading = useDelayedLoading(lazy);

  useEffect(() => {
    if (changeId) void ensureChangeLoaded(changeId);
  }, [changeId]);

  const desc = change ? change.description.split('\n')[0]
    : seedDescription ? seedDescription.split('\n')[0]
    : undefined;
  const fileCount = change?.file_count ?? seedFileCount;

  // Lifecycle error and any body content (live row OR in-thread seed) both win
  // over the lazy-fetch state — a 404 for a row that arrives via SSE moments
  // later shouldn't strand a stale "Failed to load" row, and a seeded body is
  // already complete, so the fetch failure (which only gates the footer's
  // Diff/Revert) must not surface a redundant error line beneath it.
  const lazyFailedError = !error && !desc && fileCount == null && lazy.status === 'failed'
    ? `Failed to load change details: ${lazy.error}`
    : undefined;
  // No "Loading..." line once the seed has already painted the body — the
  // fetch is still running (the footer's Diff/Revert need the live row), but
  // the body is complete, so a Loading line would just be a flicker + a shift.
  const lazyLoading = !desc && fileCount == null && lazy.status === 'loading' && showLoading;

  if (!desc && fileCount == null && !error && !lazyFailedError && !lazyLoading) return null;
  return (
    <div class="change-body">
      {desc && <div class="change-body-desc">{desc}</div>}
      {fileCount != null && (
        <div class="change-body-meta">{formatFileCount(fileCount)}</div>
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
 *  pending — user reads the error, doesn't diff/revert).
 *
 *  `reserveWhileLoading` (set for ChangeApplied panels, which always get at
 *  least a Revert button) renders a hidden button placeholder while the per-id
 *  `Change` row is still being fetched — so the real Diff/Revert buttons slot
 *  into an already-reserved footer row without a vertical shift on first open.
 *  Mirrors the body's `seed*` props in <ChangeBody>. */
export function changeActions(changeId?: string, suppress?: boolean, reserveWhileLoading?: boolean): ComponentChildren {
  if (suppress || !changeId) return null;
  const change = findChangeById(changeId);
  if (!change) {
    return reserveWhileLoading
      ? <button class="action-btn change-actions-placeholder" aria-hidden="true" tabIndex={-1}>Revert</button>
      : null;
  }
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

/** Shown in place of a collapsed panel's body (initiator OR response) so a
 *  folded turn never reads as an empty row — a muted "⋯", rendered as an
 *  accent stub bubble for user messages. Clicking it expands the panel. */
function CollapsedIndicator({ bubble = false, onToggle }: { bubble?: boolean; onToggle?: () => void }) {
  return (
    <div class={`turn-collapsed${bubble ? ' turn-collapsed-bubble' : ''}`} onClick={onToggle}>
      <span class="turn-collapsed-dots" aria-label="Collapsed — click to expand">⋯</span>
    </div>
  );
}

interface InitiatorPanelProps {
  initiator: InitiatorDescriptor;
  timestamp: string;
  onActorClick?: (e: MouseEvent) => void;
  actions?: ComponentChildren;
  collapsible: boolean;
  collapsed: boolean;
  onToggle?: () => void;
  /** User message → render the body as a right-aligned gray bubble. */
  bubble?: boolean;
  /** Drop the actor chip (icon + name) entirely — used for user messages and
   *  change-lifecycle turns. Attribution is reached via the clickable timestamp
   *  (and, for change turns, the summary line), which open the route popover. */
  chromeless?: boolean;
}

function ActorChipBody({ initiator }: { initiator: InitiatorDescriptor }) {
  return (
    <>
      {/* Iconless chips (e.g. the "Response canceled" boundary header) skip the
          icon span entirely so the label sits flush, with no leading gap. */}
      {initiator.icon != null && initiator.icon !== '' && (
        <span class="initiator-icon">{initiator.icon}</span>
      )}
      <span class="initiator-label">{initiator.label}</span>
    </>
  );
}

export function InitiatorPanel({ initiator, timestamp, onActorClick, actions, collapsible, collapsed, onToggle, bubble = false, chromeless = false }: InitiatorPanelProps) {
  const accentClass = initiator.accent ? ` initiator-panel-${initiator.accent}` : '';
  const hasBody = !!initiator.summary || !!initiator.details;
  // A chromeless turn whose summary opens the popover renders that summary as a
  // button ("Change applied" → origin info). Plain summaries stay a <div>.
  const summaryLinks = chromeless && !!onActorClick;

  return (
    <div class={`initiator-panel initiator-panel-${initiator.variant}${accentClass}${bubble ? ' initiator-panel-bubble' : ''}${collapsed ? ' initiator-panel-collapsed' : ''}`}>
      <div
        class={`initiator-header${collapsible ? ' initiator-header-clickable' : ''}`}
        onClick={(e) => handlePanelHeaderClick(e, onToggle)}
      >
        {!chromeless && (onActorClick ? (
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
        ))}
        {/* Status (question/permission resolution) + timestamp, grouped right.
            Chromeless turns have no actor chip — the timestamp is the popover
            trigger; it's a <button> so it's excluded from the collapse click. */}
        <span class="initiator-meta">
          {initiator.status}
          {chromeless && onActorClick ? (
            <button
              type="button"
              class="initiator-timestamp initiator-timestamp-button"
              onClick={onActorClick}
              aria-label={`Show info for ${initiator.label}`}
            >
              {timestamp}
            </button>
          ) : (
            <span class="initiator-timestamp">{timestamp}</span>
          )}
        </span>
      </div>
      {hasBody && !collapsed && (
        <div class="initiator-body">
          {initiator.summary && (summaryLinks ? (
            <button type="button" class="initiator-summary initiator-summary-link" onClick={onActorClick}>
              {initiator.summary}
            </button>
          ) : (
            <div class="initiator-summary">{initiator.summary}</div>
          ))}
          {bubble ? <div class="user-bubble">{initiator.details}</div> : initiator.details}
        </div>
      )}
      {hasBody && collapsed && <CollapsedIndicator bubble={bubble} onToggle={onToggle} />}
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
  codingAgent: CodingAgent = 'claude-code',
): { icon: ComponentChildren; label: string } {
  if (isCC && codingAgent === 'codex') return { icon: <CodexIcon />, label: 'Codex' };
  if (isCC) return { icon: <ClaudeIcon />, label: 'Claude Code' };
  return { icon: <LucidosGlyph />, label: LUCIDOS_AGENT_LABEL };
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
      {hasBody && collapsed && <CollapsedIndicator onToggle={onToggle} />}
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

/** Latest non-empty line of streamed reasoning, truncated — the inline "ticker"
 *  preview for a live Thinking step. The full text lives in the detail modal. */
function lastLinePreview(text: string, max = 120): string {
  const line = text.split('\n').map(s => s.trim()).filter(Boolean).pop() ?? '';
  return line.length > max ? `${line.slice(0, max - 1)}…` : line;
}

export function InlineStep({ event }: { event: Extract<ResponseEvent, { type: 'step' }> }) {
  const { className } = stepStatus(event.success);
  const snap = event.contextCapture;
  const used = snap?.usage?.input_tokens ?? snap?.estimated_total_tokens ?? event.context_tokens;
  const window = snap?.context_window;
  const trimmed = snap?.trimmed ?? event.trimmed;
  const hasContext = used != null;
  // A "Thinking" step's reasoning streams into `thinkingText`. Show its tail
  // (latest non-empty line, truncated) inline as a live ticker so a long
  // reasoning pass visibly progresses; the full text is in the detail modal.
  const thinkingTail = event.thinkingText ? lastLinePreview(event.thinkingText) : '';
  const detailText = event.detail || thinkingTail;

  return (
    <button
      type="button"
      class={`inline-step ${className}`}
      data-role="inline-step"
      onClick={() => { stepDetailModal.value = event; }}
    >
      {/* In-progress step: no leading icon — the shimmering description is the
          "live" affordance (the step-icon is hidden via CSS for `.pending`). */}
      <span class="step-icon">
        {event.success === null ? null : event.success ? '✓' : '⚠'}
      </span>
      <span class={`step-description${event.success === null ? ' running-shimmer' : ''}`}>{highlightEllipsis(event.description)}</span>
      {detailText && <span class="step-detail">{highlightEllipsis(detailText)}</span>}
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

/** The command-guard checkpoint card (ADR 0002, Phase 4): an inline affordance
 *  shown before an in-workspace destructive command, with a one-click Undo that
 *  restores the workspace from the snapshot. Once reverted (live via the paired
 *  `CommandCheckpointReverted` event, or on reload), the button is replaced with
 *  a reverted marker. */
export function CheckpointCard({ event }: { event: Extract<ResponseEvent, { type: 'checkpoint' }> }) {
  const pending = useSignal(false);
  const onUndo = async () => {
    if (pending.value || event.reverted) return;
    pending.value = true;
    try {
      // Success flips `reverted` via the SSE CommandCheckpointReverted event,
      // which re-groups into this exchange and re-renders the card.
      await postCommandCheckpointUndo(event.checkpoint_id);
    } catch (e) {
      showToast('Undo failed: ' + errorDetail(e), 'error');
    } finally {
      pending.value = false;
    }
  };
  return (
    <div class="checkpoint-card" data-role="checkpoint-card">
      <span class="checkpoint-icon" aria-hidden="true">{'⎌'}</span>
      <div class="checkpoint-body">
        <div class="checkpoint-summary">{event.summary}</div>
        <code class="checkpoint-command">{event.command}</code>
      </div>
      {event.reverted ? (
        <span class="checkpoint-reverted">Reverted ✓</span>
      ) : (
        <button
          type="button"
          class="action-btn action-btn-danger"
          data-role="checkpoint-undo"
          disabled={pending.value}
          onClick={onUndo}
        >
          {pending.value ? 'Undoing…' : 'Undo'}
        </button>
      )}
    </div>
  );
}
