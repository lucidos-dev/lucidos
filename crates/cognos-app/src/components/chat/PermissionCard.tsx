import { useSignal } from '@preact/signals';
import { showToast } from '../../store/store';
import { postMcpConsent } from '../../api/client';
import { errorDetail } from '../../utils/errorDetail';

export interface PermissionEvent {
  request_id: string;
  tool_use_id: string;
  tool_name: string;
  input: Record<string, unknown>;
  summary: string;
  resolved?: { allowed: boolean; reason?: string };
}

interface Props {
  event: PermissionEvent;
}

/** Split "skill update-config" into "skill " + <strong>update-config</strong>
 *  so the meaningful arg stands out from the tool-name prefix. */
function renderSummary(summary: string) {
  const space = summary.indexOf(' ');
  if (space === -1) return <strong>{summary}</strong>;
  return (
    <>
      {summary.slice(0, space)} <strong>{summary.slice(space + 1)}</strong>
    </>
  );
}

/** Inline card surfaced when CC's permission prompt fires. The `pending` signal
 *  is an optimistic override — replaced by `event.resolved` once the paired
 *  CodingAgentPermissionResolved event arrives over SSE. */
export function PermissionCard({ event }: Props) {
  const pending = useSignal<boolean | null>(null);

  if (event.resolved) {
    return <AnsweredCard event={event} resolved={event.resolved} />;
  }
  if (pending.value !== null) {
    return <AnsweredCard event={event} resolved={{ allowed: pending.value }} optimistic />;
  }

  const decide = async (allowed: boolean) => {
    pending.value = allowed;
    try {
      await postMcpConsent(event.request_id, allowed);
    } catch (e) {
      pending.value = null;
      showToast(`Could not send decision: ${errorDetail(e)}`, 'error');
    }
  };

  return (
    <div class="cc-permission-card" data-request-id={event.request_id}>
      <div class="cc-permission-text">
        Claude Code wants to use {renderSummary(event.summary)}. Allow?
      </div>
      <div class="cc-permission-actions">
        <button
          type="button"
          class="action-btn action-btn-confirm"
          onClick={() => decide(true)}
          aria-label="Allow this permission request"
        >
          Allow
        </button>
        <button
          type="button"
          class="action-btn action-btn-danger"
          onClick={() => decide(false)}
          aria-label="Deny this permission request"
        >
          Deny
        </button>
      </div>
    </div>
  );
}

function AnsweredCard({
  event,
  resolved,
  optimistic = false,
}: {
  event: PermissionEvent;
  resolved: { allowed: boolean; reason?: string };
  optimistic?: boolean;
}) {
  const verdict = resolved.allowed
    ? 'Allowed'
    : `Denied${resolved.reason ? `: ${resolved.reason}` : ''}`;
  return (
    <div
      class={`cc-permission-card cc-permission-card-answered${
        optimistic ? ' cc-permission-card-pending' : ''
      }`}
    >
      <div class="cc-permission-text">{renderSummary(event.summary)}</div>
      <div class="cc-permission-answer">{verdict}</div>
    </div>
  );
}
