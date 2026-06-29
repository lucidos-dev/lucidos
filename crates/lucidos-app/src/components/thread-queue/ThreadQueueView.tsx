import { useState } from 'preact/hooks';
import { threadQueue, threadMap } from '../../store/store';
import {
  dropQueueEntry,
  runQueueEntryNow,
  saveCapacityPolicy,
} from '../../store/actions/threadQueue';
import { focusThreadOrBootstrap } from '../../store/actions/threads';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import type { CapacityPolicy, ThreadQueueEntry } from '../../store/types';
import { threadDisplayTitle } from '../../utils/threadTitle';
import { formatShortDate, formatShortTime } from '../../utils/formatTime';
import { LoadableError } from '../shared/LoadableError';
import { ListSkeleton } from '../shared/ListSkeleton';
import { LoadingFade } from '../shared/LoadingFade';
import { Dropdown } from '../shared/Dropdown';

const KIND_LABELS: Record<ThreadQueueEntry['kind'], string> = {
  'event-trigger': 'Event trigger',
  cron: 'Scheduled',
  'sub-thread': 'Sub-thread',
  'coding-agent': 'Coding agent',
  'user-chat': 'You',
};

function formatTimestamp(iso: string): string {
  const d = new Date(iso);
  return `${formatShortDate(d)} ${formatShortTime(d)}`;
}

/** Open the thread an entry is bound to. Background entries without a
 *  materialized thread (a queued trigger fire that hasn't spawned one yet) have
 *  no thread_id and aren't navigable. Uses focusThreadOrBootstrap so a thread
 *  outside the loaded window (background work) still opens. Exported for the
 *  unit test. */
export function openQueueEntryThread(entry: ThreadQueueEntry): void {
  if (!entry.thread_id) return;
  focusThreadOrBootstrap(entry.thread_id);
}

function ThreadQueueItem({ entry }: { entry: ThreadQueueEntry }) {
  const running = entry.status === 'admitted';
  // User-initiated entries aren't background queue rows — Run now / Drop act on
  // the projection, which doesn't hold them. A queued user thread is already
  // prioritized; cancel it from the chat itself, not here.
  const actionable = !running && entry.kind !== 'user-chat';
  // Prefer the live thread title the user sees in the drawer / header; fall
  // back to the queued prompt for entries whose thread isn't loaded
  // (background work) or hasn't been titled yet. Reading threadMap here makes
  // the row re-render when ThreadTitleGenerated lands.
  const thread = entry.thread_id ? threadMap.value.get(entry.thread_id) : undefined;
  const title = thread ? threadDisplayTitle(thread) : entry.summary;
  const navigable = !!entry.thread_id;
  return (
    <div
      class={`list-row thread-queue-row${navigable ? ' clickable' : ''}`}
      data-role="thread-queue-row"
      onClick={navigable ? () => openQueueEntryThread(entry) : undefined}
    >
      <div class="list-row-info">
        <div class="title list-row-name">{title}</div>
        <div class="list-row-details">
          <span class={running ? 'trigger-enabled' : 'trigger-paused'}>
            {running ? 'Running' : 'Queued'}
          </span>
          <span class="label">{KIND_LABELS[entry.kind]}</span>
          {entry.trigger_name && <span class="label">{entry.trigger_name}</span>}
        </div>
        <div class="list-row-date">
          {running && entry.admitted_at
            ? `Started ${formatTimestamp(entry.admitted_at)}`
            : `Queued ${formatTimestamp(entry.queued_at)}`}
        </div>
      </div>
      {actionable && (
        <div class="list-row-actions">
          <button
            class="action-btn action-btn-danger"
            onClick={(e) => {
              e.stopPropagation();
              void dropQueueEntry(entry.id, entry.summary);
            }}
          >
            Drop
          </button>
          <button
            class="action-btn action-btn-confirm"
            onClick={(e) => {
              e.stopPropagation();
              void runQueueEntryNow(entry.id);
            }}
          >
            Run now
          </button>
        </div>
      )}
    </div>
  );
}

/** Numeric policy fields rendered as labeled inputs, in display order. */
const POLICY_FIELDS: Array<{ key: keyof CapacityPolicy & string; label: string }> = [
  { key: 'max_concurrent_total', label: 'Max concurrent (total)' },
  { key: 'reserved_background', label: 'Reserved for background' },
  { key: 'max_concurrent_event_trigger', label: 'Max concurrent event triggers' },
  { key: 'max_concurrent_cron', label: 'Max concurrent scheduled' },
  { key: 'max_concurrent_sub_thread', label: 'Max concurrent sub-threads' },
  { key: 'max_concurrent_coding_agent', label: 'Max concurrent coding agents' },
  { key: 'max_concurrent_per_trigger', label: 'Max concurrent per trigger' },
  { key: 'max_queued_per_trigger', label: 'Max queued per trigger' },
];

function CapacityPolicyEditor({ policy }: { policy: CapacityPolicy }) {
  const [draft, setDraft] = useState<CapacityPolicy>(policy);
  const [saving, setSaving] = useState(false);
  const dirty = JSON.stringify(draft) !== JSON.stringify(policy);

  async function save() {
    setSaving(true);
    const ok = await saveCapacityPolicy(draft);
    setSaving(false);
    if (!ok) return;
  }

  return (
    <div class="thread-queue-policy" data-role="capacity-policy">
      <div class="thread-queue-policy-grid">
        {POLICY_FIELDS.map(({ key, label }) => (
          <label class="thread-queue-policy-field" key={key}>
            <span>{label}</span>
            <input
              type="number"
              min={key === 'max_queued_per_trigger' ? 1 : 0}
              value={draft[key] as number}
              onInput={(e) => {
                const n = parseInt((e.target as HTMLInputElement).value, 10);
                if (!Number.isNaN(n) && n >= 0) setDraft({ ...draft, [key]: n });
              }}
            />
          </label>
        ))}
        <label class="thread-queue-policy-field">
          <span>On per-trigger queue overflow</span>
          <Dropdown
            options={[
              { value: 'drop-oldest', label: 'Drop oldest + notify' },
              { value: 'pause-trigger', label: 'Pause trigger + notify' },
            ]}
            value={draft.overflow}
            onChange={(value) =>
              setDraft({ ...draft, overflow: value as CapacityPolicy['overflow'] })
            }
          />
        </label>
      </div>
      <div class="thread-queue-policy-actions">
        <button
          class="action-btn"
          disabled={!dirty || saving}
          onClick={() => setDraft(policy)}
        >
          Reset
        </button>
        <button
          class="action-btn action-btn-confirm"
          disabled={!dirty || saving}
          onClick={() => void save()}
        >
          Save policy
        </button>
      </div>
    </div>
  );
}

export function ThreadQueueView() {
  const loadable = threadQueue.value;
  const showLoading = useDelayedLoading(loadable);
  const [policyOpen, setPolicyOpen] = useState(false);

  if (loadable.status === 'failed') {
    return (
      <div class="content-view active">
        <div class="list-rows">
          <LoadableError noun="thread queue" error={loadable.error} />
        </div>
      </div>
    );
  }

  return (
    <div class="content-view active">
      <div class="list-rows">
        <LoadingFade showSkeleton={showLoading} skeleton={<ListSkeleton fill />}>
          {loadable.status === 'loaded'
            ? (() => {
                const { entries, policy } = loadable.data;
                const running = entries.filter((e) => e.status === 'admitted');
                const queued = entries.filter((e) => e.status === 'queued');
                return (
                  <>
                    <div class="thread-queue-section-header">
                      <span class="thread-queue-section-title">
                        Running ({running.length}/{policy.max_concurrent_total})
                      </span>
                      <button
                        class="action-btn"
                        data-role="toggle-capacity-policy"
                        onClick={() => setPolicyOpen(!policyOpen)}
                      >
                        {policyOpen ? 'Hide policy' : 'Capacity policy'}
                      </button>
                    </div>
                    {policyOpen && <CapacityPolicyEditor policy={policy} />}
                    {running.length === 0 && (
                      <div class="empty thread-queue-empty">Nothing running.</div>
                    )}
                    {running.map((entry) => (
                      <ThreadQueueItem key={entry.id} entry={entry} />
                    ))}

                    <div class="thread-queue-section-header">
                      <span class="thread-queue-section-title">Queued ({queued.length})</span>
                    </div>
                    {queued.length === 0 && (
                      <div class="empty thread-queue-empty" data-role="thread-queue-empty">
                        Nothing waiting — threads run immediately while capacity is free.
                      </div>
                    )}
                    {queued.map((entry) => (
                      <ThreadQueueItem key={entry.id} entry={entry} />
                    ))}
                  </>
                );
              })()
            : null}
        </LoadingFade>
      </div>
    </div>
  );
}
