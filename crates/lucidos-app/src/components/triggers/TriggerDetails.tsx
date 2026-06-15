import { useState, useEffect, useRef } from 'preact/hooks';
import { activeInlineForm, triggers, triggerGroups, showToast } from '../../store/store';
import {
  closeTriggerForm,
  submitTrigger,
} from '../../store/actions/triggers';
import { createTriggerGroup } from '../../store/actions/triggerGroups';
import { deriveTriggerType, toFailed } from '../../store/types';
import type { EventSubscription, SideEffectCategory, TriggerInfo, TriggerRun, Loadable } from '../../store/types';
import { describeCron, validateCron } from '../../utils/describeCron';
import { Dropdown } from '../shared/Dropdown';
import { fetchEventTypes } from '../../api/client';
import { resizeTextarea, useFontMetricsResize } from '../chat/promptResize';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { LoadableError } from '../shared/LoadableError';
import { DelayedSpinner } from '../shared/DelayedSpinner';

const NEW_GROUP_SENTINEL = '__new_group__';

type TriggerFormType = 'schedule' | 'event' | 'both';
type RunType = 'intent' | 'script';

/** The grantable irreversible side-effect categories, with their user-facing
 *  labels (ADR 0002, Phase 5). Kept in sync with the Rust `SideEffectCategory`
 *  enum's `label()` and the `SideEffectCategory` wire type. */
const SIDE_EFFECT_CATEGORIES: { value: SideEffectCategory; label: string }[] = [
  { value: 'email', label: 'Send email or messages' },
  { value: 'external_api', label: 'Call external APIs (mutating HTTP)' },
  { value: 'cloud_cli', label: 'Cloud CLI mutations (gh / aws / gcloud)' },
  { value: 'out_of_workspace_destruction', label: 'Destroy files outside the workspace' },
  { value: 'other', label: 'Other irreversible side-effects' },
];

// Module-level cache: shared across all TriggerFormInner mounts (open/reopen
// the form without refetching). Loadable so consumers see all 4 states.
let cachedEventTypes: Loadable<string[]> = { status: 'not-loaded' };
let inflightFetch: Promise<string[]> | null = null;

export function TriggerDetails() {
  const form = activeInlineForm.value;
  if (form?.type !== 'trigger') return null;

  const editingId = form.triggerId;

  if (editingId) {
    if (triggers.value.status === 'failed') {
      return (
        <div class="inline-form">
          <LoadableError error={triggers.value.error} noun="triggers" />
        </div>
      );
    }
    if (triggers.value.status !== 'loaded') {
      return <div class="inline-form"><DelayedSpinner /></div>;
    }
    const trigger = triggers.value.data.find((t) => t.id === editingId);
    if (!trigger) {
      return <MissingTriggerCloser />;
    }
    return <TriggerFormInner key={editingId} editingId={editingId} existingTrigger={trigger} />;
  }

  return <TriggerFormInner key="new" existingTrigger={null} />;
}

/** Closes the trigger form when the target trigger no longer exists. Lives in
 *  a child component so the signal write happens in useEffect (post-commit),
 *  not inside the parent's render body — which preact lint flags. */
function MissingTriggerCloser() {
  useEffect(() => { closeTriggerForm(); }, []);
  return null;
}

function TriggerFormInner({ editingId, existingTrigger }: { editingId?: string; existingTrigger: TriggerInfo | null }) {

  const derived = existingTrigger ? deriveTriggerType(existingTrigger) : null;
  const initialFormType: TriggerFormType = derived === 'hybrid' ? 'both' : derived === 'event' ? 'event' : 'schedule';

  const [formType, setFormType] = useState<TriggerFormType>(initialFormType);
  const [name, setName] = useState(existingTrigger?.name || '');

  const existingRun = existingTrigger?.run;
  const [runType, setRunType] = useState<RunType>(existingRun?.type === 'script' ? 'script' : 'intent');
  const [intentText, setIntentText] = useState(
    existingRun?.type === 'intent' ? existingRun.intent : ''
  );
  const [scriptPath, setScriptPath] = useState(
    existingRun?.type === 'script' ? existingRun.path : ''
  );

  const [cronExpressions, setCronExpressions] = useState<string[]>(
    existingTrigger?.cron_expressions?.length ? [...existingTrigger.cron_expressions] : []
  );
  const [cronInput, setCronInput] = useState('');
  const [cronError, setCronError] = useState<string | null>(null);

  // Parsed condition lives on `subs[i].condition` so submit can forward it
  // directly. The raw JSON source the user is typing lives in `conditionDrafts`
  // — parsing it back on every keystroke would lose the in-flight cursor /
  // invalid intermediate state, so we hold the string until submit.
  const initialSubs: EventSubscription[] = (existingTrigger?.on ?? []).map(s => ({ ...s }));
  const initialDrafts: Record<number, string> = {};
  (existingTrigger?.on ?? []).forEach((s, i) => {
    if (s.condition) initialDrafts[i] = JSON.stringify(s.condition, null, 2);
  });
  const [subs, setSubs] = useState<EventSubscription[]>(initialSubs);
  const [conditionDrafts, setConditionDrafts] = useState<Record<number, string>>(initialDrafts);
  const [subErrors, setSubErrors] = useState<Record<number, string>>({});
  const [eventTypesLoadable, setEventTypesLoadable] = useState<Loadable<string[]>>(cachedEventTypes);
  const showEventTypesLoading = useDelayedLoading(eventTypesLoadable);

  useEffect(() => {
    if (cachedEventTypes.status === 'loaded') return;
    setEventTypesLoadable({ status: 'loading' });
    cachedEventTypes = { status: 'loading' };
    if (!inflightFetch) {
      inflightFetch = fetchEventTypes();
    }
    inflightFetch
      .then(types => {
        cachedEventTypes = { status: 'loaded', data: types };
        setEventTypesLoadable(cachedEventTypes);
      })
      .catch((e: unknown) => {
        cachedEventTypes = toFailed(e);
        setEventTypesLoadable(cachedEventTypes);
      })
      .finally(() => { inflightFetch = null; });
  }, []);

  const eventTypeOptions: { value: string; label: string }[] = (() => {
    if (eventTypesLoadable.status === 'loaded') {
      return eventTypesLoadable.data.map(t => ({ value: t, label: t }));
    }
    if (eventTypesLoadable.status === 'failed') {
      return [{ value: '', label: 'Failed to load event types' }];
    }
    if (eventTypesLoadable.status === 'loading' && showEventTypesLoading) {
      return [{ value: '', label: 'Loading event types...' }];
    }
    return [];
  })();

  const [goToReview, setGoToReview] = useState(existingTrigger?.go_to_review ?? false);
  const [sideEffectGrant, setSideEffectGrant] = useState<SideEffectCategory[]>(
    existingTrigger?.side_effect_grant ?? []
  );
  const toggleSideEffect = (cat: SideEffectCategory, on: boolean) => {
    setSideEffectGrant(prev =>
      on ? [...prev.filter(c => c !== cat), cat] : prev.filter(c => c !== cat)
    );
  };
  const [groupId, setGroupId] = useState<string>(existingTrigger?.group_id ?? '');
  // null = inline-create field hidden; string = visible with current draft.
  const [newGroupDraft, setNewGroupDraft] = useState<string | null>(null);

  const intentRef = useRef<HTMLTextAreaElement>(null);
  const resizeIntent = () => { if (intentRef.current) resizeTextarea(intentRef.current); };
  // runType included in deps because the textarea is unmounted/remounted when switching to script.
  useEffect(resizeIntent, [intentText, runType]);
  useFontMetricsResize(resizeIntent);

  const showCron = formType === 'schedule' || formType === 'both';
  const showEvent = formType === 'event' || formType === 'both';

  function addCron() {
    const trimmed = cronInput.trim();
    if (!trimmed) return;

    const error = validateCron(trimmed);
    if (error) {
      setCronError(error);
      return;
    }

    setCronExpressions([...cronExpressions, trimmed]);
    setCronInput('');
    setCronError(null);
  }

  function removeCron(index: number) {
    setCronExpressions(cronExpressions.filter((_, i) => i !== index));
  }

  function handleCronKeyDown(e: KeyboardEvent) {
    if (e.key === 'Enter') {
      e.preventDefault();
      addCron();
    }
  }

  function buildRun(): TriggerRun | null {
    if (runType === 'intent') {
      if (!intentText.trim()) {
        showToast('Intent text is required', 'error');
        return null;
      }
      return { type: 'intent', intent: intentText.trim() };
    }
    if (!scriptPath.trim()) {
      showToast('Script path is required', 'error');
      return null;
    }
    return { type: 'script', path: scriptPath.trim() };
  }

  async function handleSubmit(e: Event) {
    e.preventDefault();

    const run = buildRun();
    if (!run) return;

    // If there's text in the input, try to add it first
    let finalCrons = showCron ? cronExpressions : [];
    if (showCron && cronInput.trim()) {
      const error = validateCron(cronInput.trim());
      if (error) {
        setCronError(error);
        return;
      }
      finalCrons = [...cronExpressions, cronInput.trim()];
      setCronExpressions(finalCrons);
      setCronInput('');
      setCronError(null);
    }

    let on: EventSubscription[] | undefined;
    if (showEvent) {
      const built: EventSubscription[] = [];
      const errors: Record<number, string> = {};
      subs.forEach((row, i) => {
        const eventType = row.event_type.trim();
        if (!eventType) return;
        const draft = (conditionDrafts[i] ?? '').trim();
        let condition: Record<string, unknown> | undefined;
        if (draft) {
          try {
            condition = JSON.parse(draft);
          } catch {
            errors[i] = 'Invalid JSON';
          }
        }
        built.push(condition ? { event_type: eventType, condition } : { event_type: eventType });
      });
      if (Object.keys(errors).length > 0) {
        setSubErrors(errors);
        showToast('Fix the highlighted condition JSON before saving', 'error');
        return;
      }
      setSubErrors({});
      on = built;
    }

    await submitTrigger({
      name, run, cronExpressions: finalCrons, triggerId: editingId,
      on, showEvent, goToReview,
      groupId: groupId || null,
      sideEffectGrant,
    });
  }

  function clearSubError(index: number) {
    if (!subErrors[index]) return;
    const next = { ...subErrors };
    delete next[index];
    setSubErrors(next);
  }

  function addSubscription() {
    setSubs([...subs, { event_type: '' }]);
  }

  function setEventType(index: number, eventType: string) {
    setSubs(subs.map((s, i) => i === index ? { ...s, event_type: eventType } : s));
    clearSubError(index);
  }

  function setConditionDraft(index: number, source: string) {
    setConditionDrafts({ ...conditionDrafts, [index]: source });
    clearSubError(index);
  }

  function removeSubscription(index: number) {
    setSubs(subs.filter((_, i) => i !== index));
    // Rekey both maps so trailing entries shift down with the array.
    const reindex = <T,>(m: Record<number, T>) => Object.fromEntries(
      Object.entries(m)
        .map(([k, v]) => [Number(k), v] as const)
        .filter(([k]) => k !== index)
        .map(([k, v]) => [k > index ? k - 1 : k, v]),
    );
    setConditionDrafts(reindex(conditionDrafts));
    setSubErrors(reindex(subErrors));
  }

  function handleGroupChange(value: string) {
    if (value === NEW_GROUP_SENTINEL) {
      setNewGroupDraft('');
      return;
    }
    setNewGroupDraft(null);
    setGroupId(value);
  }

  async function commitNewGroupDraft() {
    const trimmed = (newGroupDraft ?? '').trim();
    if (!trimmed) { setNewGroupDraft(null); return; }
    const group = await createTriggerGroup(trimmed);
    if (group) setGroupId(group.id);
    setNewGroupDraft(null);
  }

  const groupsLoadable = triggerGroups.value;
  const groupOptions: { value: string; label: string }[] = (() => {
    const opts: { value: string; label: string }[] = [{ value: '', label: '(No group)' }];
    if (groupsLoadable.status === 'loaded') {
      for (const g of [...groupsLoadable.data].sort((a, b) => a.order - b.order)) {
        opts.push({ value: g.id, label: g.name });
      }
    }
    opts.push({ value: NEW_GROUP_SENTINEL, label: '+ New group…' });
    return opts;
  })();

  return (
    <div class="inline-form">
      <form onSubmit={handleSubmit}>
        <div class="inline-form-body">
          <div class="form-group">
            <label>Trigger Name</label>
            <input
              type="text"
              value={name}
              onInput={(e) => setName((e.target as HTMLInputElement).value)}
              placeholder="e.g. Morning Brief"
              required
            />
          </div>

          <div class="form-group">
            <label>Group</label>
            <Dropdown
              value={groupId}
              onChange={handleGroupChange}
              options={groupOptions}
            />
            {newGroupDraft !== null && (
              <input
                class="trigger-group-name-input"
                type="text"
                autoFocus
                value={newGroupDraft}
                placeholder="New group name"
                onInput={e => setNewGroupDraft((e.target as HTMLInputElement).value)}
                onBlur={commitNewGroupDraft}
                onKeyDown={e => {
                  if (e.key === 'Enter') { e.preventDefault(); void commitNewGroupDraft(); }
                  else if (e.key === 'Escape') setNewGroupDraft(null);
                }}
              />
            )}
            <div class="form-hint">
              Optional folder shown in the triggers panel. Group related triggers (e.g. steps of a workflow connected by emit_event → on_event).
            </div>
          </div>

          <div class="form-group">
            <label>Trigger Type</label>
            <div class="segmented-control">
              <button
                type="button"
                class={`segmented-btn ${formType === 'schedule' ? 'active' : ''}`}
                onClick={() => setFormType('schedule')}
              >
                Schedule
              </button>
              <button
                type="button"
                class={`segmented-btn ${formType === 'event' ? 'active' : ''}`}
                onClick={() => setFormType('event')}
              >
                Event
              </button>
              <button
                type="button"
                class={`segmented-btn ${formType === 'both' ? 'active' : ''}`}
                onClick={() => setFormType('both')}
              >
                Both
              </button>
            </div>
          </div>

          {showCron && (
            <div class="form-group">
              <label>Schedule</label>

              {cronExpressions.length > 0 && (
                <ul class="removable-list">
                  {cronExpressions.map((expr, i) => (
                    <li key={i} class="removable-list-item">
                      <div class="removable-list-item-info">
                        <span class="cron-description">{describeCron(expr)}</span>
                        <code class="cron-expression">{expr}</code>
                      </div>
                      <button
                        type="button"
                        class="action-btn action-btn-danger"
                        onClick={() => removeCron(i)}
                      >
                        Remove
                      </button>
                    </li>
                  ))}
                </ul>
              )}

              <div class="removable-input-row">
                <input
                  type="text"
                  value={cronInput}
                  onInput={(e) => {
                    setCronInput((e.target as HTMLInputElement).value);
                    setCronError(null);
                  }}
                  onKeyDown={handleCronKeyDown}
                  placeholder="0 0 8 * * *"
                  class={cronError ? 'input-error' : ''}
                />
                <button
                  type="button"
                  class="action-btn"
                  onClick={addCron}
                >
                  Add
                </button>
              </div>

              {cronError && <div class="form-error">{cronError}</div>}
              {cronInput.trim() && !cronError && (
                <div class="form-hint">{describeCron(cronInput.trim())}</div>
              )}
              <div class="form-hint">
                Format: sec min hour day month weekday — or ask Lucidos to set up the schedule for you.
              </div>
            </div>
          )}

          {showEvent && (
            <div class="form-group">
              <label>Event Subscriptions</label>
              {eventTypesLoadable.status === 'failed' && (
                <div class="form-error">Failed to load event types: {eventTypesLoadable.error}</div>
              )}
              {eventTypesLoadable.status === 'loading' && showEventTypesLoading && (
                <div class="form-hint">Loading event types...</div>
              )}

              {subs.length === 0 && (
                <div class="form-hint">No event subscriptions yet — add one below.</div>
              )}

              {subs.map((row, i) => (
                <div key={i} class="trigger-subscription-row">
                  <div class="removable-input-row">
                    <Dropdown
                      options={eventTypeOptions}
                      value={row.event_type}
                      onChange={(v) => setEventType(i, v)}
                      placeholder="e.g. OuraSleepImported"
                      freeText
                    />
                    <button
                      type="button"
                      class="action-btn action-btn-danger"
                      onClick={() => removeSubscription(i)}
                    >
                      Remove
                    </button>
                  </div>
                  <textarea
                    value={conditionDrafts[i] ?? ''}
                    onInput={(e) => setConditionDraft(i, (e.target as HTMLTextAreaElement).value)}
                    placeholder={'Condition (optional). e.g. {"sleep_score": {"$lt": 70}}'}
                    class={`code-textarea${subErrors[i] ? ' input-error' : ''}`}
                    rows={2}
                  />
                  {subErrors[i] && <div class="form-error">{subErrors[i]}</div>}
                </div>
              ))}

              <div class="removable-input-row">
                <button
                  type="button"
                  class="action-btn"
                  onClick={addSubscription}
                >
                  Add event
                </button>
              </div>
              <div class="form-hint">
                Each subscription's condition only filters that event — different events can carry different payloads.
              </div>
            </div>
          )}

          <div class="form-group">
            <label>Run</label>
            <div class="segmented-control">
              <button
                type="button"
                class={`segmented-btn ${runType === 'intent' ? 'active' : ''}`}
                onClick={() => setRunType('intent')}
              >
                Intent
              </button>
              <button
                type="button"
                class={`segmented-btn ${runType === 'script' ? 'active' : ''}`}
                onClick={() => setRunType('script')}
              >
                Script
              </button>
            </div>
          </div>

          {runType === 'intent' ? (
            <div class="form-group">
              <label>Intent</label>
              <div class="prompt-box">
                <textarea
                  ref={intentRef}
                  class="prompt-textarea"
                  value={intentText}
                  onInput={(e) => setIntentText((e.target as HTMLTextAreaElement).value)}
                  placeholder="e.g. Check my calendar and send me a summary of today's events"
                  rows={1}
                />
              </div>
            </div>
          ) : (
            <div class="form-group">
              <label>Script Path</label>
              <input
                type="text"
                value={scriptPath}
                onInput={(e) => setScriptPath((e.target as HTMLInputElement).value)}
                placeholder="e.g. oura/run.py"
              />
            </div>
          )}

          <div class="form-group">
            <label class="form-checkbox-row">
              <input
                type="checkbox"
                checked={goToReview}
                onChange={(e) => setGoToReview((e.target as HTMLInputElement).checked)}
              />
              <span>Send to Review on completion</span>
            </label>
            <div class="form-hint">
              By default, runs land in Archive. Turn this on for triggers whose output you're meant to read — daily summaries, alerts, scheduled reports.
            </div>
          </div>

          <div class="form-group">
            <label>Allowed side-effects</label>
            <div class="form-checkbox-list">
              {SIDE_EFFECT_CATEGORIES.map((cat) => (
                <label class="form-checkbox-row" key={cat.value}>
                  <input
                    type="checkbox"
                    checked={sideEffectGrant.includes(cat.value)}
                    onChange={(e) =>
                      toggleSideEffect(cat.value, (e.target as HTMLInputElement).checked)
                    }
                  />
                  <span>{cat.label}</span>
                </label>
              ))}
            </div>
            <div class="form-hint">
              Only used when Command Safety is on (Settings → Permissions). This trigger runs unattended, so it can't be asked to approve a risky command. Grant only the irreversible side-effects its intent genuinely needs — anything else is blocked and the run fails. Leave all off if it only reads, computes, or writes inside the workspace.
            </div>
          </div>

          <div class="form-actions">
            <button
              type="button"
              class="btn-cancel"
              onClick={closeTriggerForm}
            >
              Cancel
            </button>
            <button type="submit" class="btn-save">
              Save
            </button>
          </div>
        </div>
      </form>
    </div>
  );
}
