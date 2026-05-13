import { useState } from 'preact/hooks';
import { stepDetailModal } from '../../store/store';
import { ModalOverlay } from '../shared/ModalOverlay';
import { formatMessageTimestamp } from '../../utils/formatTime';
import { stepStatus } from '../../store/thread-events';
import type { ContextSection, ContextCapture } from '../../store/types';
import { formatTokens, estimateTokens, contextPercent } from '../../utils/formatTokens';
import { highlightEllipsis } from './highlightEllipsis';

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
      <div class="step-detail-context-meta">
        <code>{snap.model || '(unknown model)'}</code>
        <span> · {snap.producer === 'claude_code' ? 'Claude Code' : 'Main LLM'}</span>
        <span> · {snap.tools.length} tools</span>
        {snap.legacy && <span class="context-legacy-badge" data-tooltip="Synthesized from legacy events">legacy capture</span>}
      </div>
      <div class="context-sections">
        {snap.sections.map(s => <ContextSectionRow key={s.name} section={s} />)}
      </div>
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
    <ModalOverlay onClose={close} class="step-detail-overlay">
      <div class="step-detail-modal" role="dialog" aria-modal="true" data-role="context-captured-modal" onClick={(e) => e.stopPropagation()}>
        <div class="step-detail-header">
          <span class={`step-detail-status ${status.className}`}>{status.label}</span>
          {step.created && (
            <span class="step-detail-timestamp">{formatMessageTimestamp(step.created)}</span>
          )}
        </div>
        <div class="step-detail-description">{highlightEllipsis(step.description)}</div>
        {step.detail && <div class="step-detail-detail">{highlightEllipsis(step.detail)}</div>}
        {showFull && <pre class="step-detail-full">{step.full}</pre>}
        {step.result && (
          <>
            <div class="step-detail-section-label">Result</div>
            <pre class="step-detail-result">{step.result}</pre>
          </>
        )}
        {snap
          ? <ContextCapturePanel snap={snap} />
          : <div class="step-detail-empty">No context snapshot captured for this step.</div>}
        <button class="action-btn step-detail-close" onClick={close}>Close</button>
      </div>
    </ModalOverlay>
  );
}
