import { useRef } from 'preact/hooks';
import { hardenThread } from '../../api/client';
import type { UnproposedReason } from '../../generated/thread-event-wire';
import { focusedThreadId, notReadyReason, showToast, threadMap } from '../../store/store';
import { errorDetail } from '../../utils/errorDetail';
import { Disclosure } from '../shared/Disclosure';
import { continueStoppedThread } from './continueStoppedThread';

/** What the strip says for each reason a turn end withheld the work (ADR 0400).
 *  `hint` sits on the strip, and a tap toasts `detail`. */
export const NOT_READY_COPY: Record<UnproposedReason, { label: string; hint: string; detail: string }> = {
  plan_missing: {
    label: 'Not ready: needs a plan',
    hint: 'Ask the agent for an implementation plan, then approve it.',
    detail:
      'Lucidos held this work back because its branch has no implementation plan. Ask the agent for a plan and approve it. Its next turn end then proposes the change.',
  },
  plan_awaiting_approval: {
    label: 'Not ready: plan awaits approval',
    hint: 'Approve the plan, and the agent finishes the work.',
    detail:
      'Lucidos held this work back because its implementation plan is not approved yet. Approve the plan, and the agent proposes the change once it finishes.',
  },
  outside_bound: {
    label: 'Not ready: outside its fix bound',
    hint: 'The fix changes files its security marker did not name.',
    detail:
      'Lucidos held this work back because the security fix reaches past the files it named. The agent can drop the extra files, name them all, or write a plan for you to approve.',
  },
  hardening_missing: {
    label: 'Not ready: not hardened',
    hint: 'Harden runs the checks, then proposes the change.',
    detail:
      'Lucidos held this work back because it has not been hardened, so Apply would have to harden it first. Harden reviews the work and runs its tests. When it finishes, Lucidos proposes the change.',
  },
  turn_incomplete: {
    label: 'Not ready: the turn did not finish',
    hint: 'Continue resumes it.',
    detail:
      'The turn that made this work was stopped or cut short, so Lucidos did not propose it. Continue resumes the turn, and a turn that finishes proposes the change.',
  },
};

/** The amber strip above the prompt box that says why the focused thread's
 *  work is not ready, where Apply would otherwise be offered. */
export function NotReadyStrip() {
  const threadId = focusedThreadId.value;
  const reason = notReadyReason(threadId ? threadMap.value.get(threadId) : undefined);
  // The last reason shown, so the strip keeps its words while it rolls shut.
  const shown = useRef<{ threadId: string; reason: UnproposedReason } | null>(null);
  if (threadId && reason) shown.current = { threadId, reason };
  const last = shown.current;
  return (
    <Disclosure open={reason !== null}>
      {last && (
        <div class="not-ready-strip" data-reason={last.reason}>
          <button
            class="not-ready-strip-reason"
            onClick={() => showToast(NOT_READY_COPY[last.reason].detail, 'info', { title: NOT_READY_COPY[last.reason].label })}
          >
            <WarningIcon />
            <span class="not-ready-strip-text">
              <span class="not-ready-strip-label">{NOT_READY_COPY[last.reason].label}</span>
              <span class="not-ready-strip-hint">{` · ${NOT_READY_COPY[last.reason].hint}`}</span>
            </span>
          </button>
          {last.reason === 'turn_incomplete' && (
            <button class="action-btn protected-surface" onClick={() => void continueStoppedThread(last.threadId)}>
              Continue
            </button>
          )}
          {last.reason === 'hardening_missing' && (
            <button class="action-btn protected-surface" onClick={() => void startHardening(last.threadId)}>
              Harden
            </button>
          )}
        </div>
      )}
    </Disclosure>
  );
}

/** Start the hardening run the strip offers. A failure toasts, since the
 *  strip stays up and the press can be retried. */
async function startHardening(threadId: string): Promise<void> {
  try {
    await hardenThread(threadId);
  } catch (err) {
    showToast(`Failed to start hardening: ${errorDetail(err)}`, 'error');
  }
}

function WarningIcon() {
  return (
    <svg class="not-ready-strip-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
      <path d="M10.29 3.86 1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z" />
      <line x1="12" y1="9" x2="12" y2="13" />
      <line x1="12" y1="17" x2="12.01" y2="17" />
    </svg>
  );
}
