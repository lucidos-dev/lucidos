import { AllowlistRow } from '../shared/AllowlistRow';
import { useState, useEffect, useRef } from 'preact/hooks';
import { activeInlineForm, triggers, triggerGroups, showToast, chatModels } from '../../store/store';
import {
  closeTriggerForm,
  revokeTriggerDeny,
  revokeTriggerGrant,
  submitTrigger,
} from '../../store/actions/triggers';
import {
  chatModelOptions, loadChatModels, lucidosModelChoices, LUCIDOS_TIER_VOCABULARY,
  PROVIDER_DEFAULT_EFFORT_LABEL,
} from '../../store/actions/models';
import { accountChatEffort, currentTriggerModel } from '../../store/actions/preferences';
import type { ModelChoice } from '../../store/modelSelection';
import { ModelSelectionField } from '../shared/ModelSelectionField';
import { REASONING_LEVELS } from '../../store/models';
import { createTriggerGroup } from '../../store/actions/triggerGroups';
import { deriveTriggerType, toFailed } from '../../store/types';
import type { EventSubscription, TriggerInfo, TriggerRun, Loadable } from '../../store/types';
import { describeCron, validateCron } from '../../utils/describeCron';
import { Dropdown } from '../shared/Dropdown';
import { fetchEventTypes } from '../../api/client';
import { resizeTextarea, useFontMetricsResize, useWidthRemeasure } from '../chat/promptResize';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { useServerBackedField, sameJson } from '../../hooks/useServerBackedField';
import { LoadableError } from '../shared/LoadableError';
import { FormSkeleton } from '../shared/FormSkeleton';
import { LoadingFade } from '../shared/LoadingFade';
import { Explainer, FieldLabel } from '../shared/Explainer';
import { PROSE_TEXT_ATTRS } from '../../utils/noAutofill';

const NEW_GROUP_SENTINEL = '__new_group__';

type TriggerFormType = 'schedule' | 'event' | 'both';
type RunType = 'intent' | 'script';

let cachedEventTypes: Loadable<string[]> = { status: 'not-loaded' };
let inflightFetch: Promise<string[]> | null = null;

function loadEventTypes(apply: (l: Loadable<string[]>) => void): void {
  cachedEventTypes = { status: 'loading' };
  apply(cachedEventTypes);
  if (!inflightFetch) {
    inflightFetch = fetchEventTypes();
  }
  inflightFetch
    .then(types => {
      cachedEventTypes = { status: 'loaded', data: types };
      apply(cachedEventTypes);
    })
    .catch((e: unknown) => {
      cachedEventTypes = toFailed(e);
      apply(cachedEventTypes);
    })
    .finally(() => { inflightFetch = null; });
}

function retryFailedEventTypes(apply: (l: Loadable<string[]>) => void): void {
  if (cachedEventTypes.status === 'failed') loadEventTypes(apply);
}

interface SubscriptionDraft {
  subs: EventSubscription[];
  drafts: Record<number, string>;
}

function servedSubscriptions(on: EventSubscription[] | undefined): SubscriptionDraft {
  const subs = (on ?? []).map(s => ({ ...s }));
  const drafts: Record<number, string> = {};
  subs.forEach((s, i) => {
    if (s.condition) drafts[i] = JSON.stringify(s.condition, null, 2);
  });
  return { subs, drafts };
}

function reindexAfterRemoval<T>(map: Record<number, T>, index: number): Record<number, T> {
  return Object.fromEntries(
    Object.entries(map)
      .map(([k, v]) => [Number(k), v] as const)
      .filter(([k]) => k !== index)
      .map(([k, v]) => [k > index ? k - 1 : k, v]),
  );
}

const TRIGGER_FORM_FIELDS = [{ label: 'Trigger Name' }, { label: 'Trigger Type' }, { label: 'Run' }, { label: 'Intent', tall: true }];

export function TriggerDetails() {
  const form = activeInlineForm.value;
  const showLoading = useDelayedLoading(triggers.value);
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
    const list = triggers.value.status === 'loaded' ? triggers.value.data : null;
    const trigger = list?.find((t) => t.id === editingId);
    if (list && !trigger) {
      return <MissingTriggerCloser key={editingId} />;
    }
    return (
      <LoadingFade showSkeleton={showLoading} skeleton={<FormSkeleton fields={TRIGGER_FORM_FIELDS} />}>
        {trigger && <TriggerFormInner key={editingId} editingId={editingId} existingTrigger={trigger} />}
      </LoadingFade>
    );
  }

  return <TriggerFormInner key="new" existingTrigger={null} />;
}

function MissingTriggerCloser() {
  useEffect(() => { closeTriggerForm(); }, []);
  return null;
}

function TriggerFormInner({ editingId, existingTrigger }: { editingId?: string; existingTrigger: TriggerInfo | null }) {

  const derived = existingTrigger ? deriveTriggerType(existingTrigger) : null;
  const servedFormType: TriggerFormType = derived === 'hybrid' ? 'both' : derived === 'event' ? 'event' : 'schedule';

  const [formType, setFormType] = useServerBackedField<TriggerFormType>(servedFormType);
  const [name, setName] = useServerBackedField(existingTrigger?.name || '');

  const existingRun = existingTrigger?.run;
  const [runType, setRunType] = useServerBackedField<RunType>(
    existingRun?.type === 'script' ? 'script' : 'intent'
  );
  const [intentText, setIntentText] = useServerBackedField(
    existingRun?.type === 'intent' ? existingRun.intent : ''
  );
  const [scriptPath, setScriptPath] = useServerBackedField(
    existingRun?.type === 'script' ? existingRun.path : ''
  );

  const [cronExpressions, setCronExpressions] = useServerBackedField<string[]>(
    existingTrigger?.cron_expressions ?? [], sameJson,
  );
  const [cronInput, setCronInput] = useState('');
  const [cronError, setCronError] = useState<string | null>(null);

  const [subscriptions, setSubscriptions] = useServerBackedField(
    servedSubscriptions(existingTrigger?.on), sameJson,
  );
  const { subs, drafts: conditionDrafts } = subscriptions;
  const [subErrors, setSubErrors] = useState<Record<number, string>>({});
  const [eventTypesLoadable, setEventTypesLoadable] = useState<Loadable<string[]>>(cachedEventTypes);
  const showEventTypesLoading = useDelayedLoading(eventTypesLoadable);

  useEffect(() => {
    if (cachedEventTypes.status === 'loaded') return;
    loadEventTypes(setEventTypesLoadable);
  }, []);

  const eventTypeOptions: { value: string; label: string }[] = (() => {
    if (eventTypesLoadable.status === 'loaded') {
      return eventTypesLoadable.data.map(t => ({ value: t, label: t }));
    }
    if (eventTypesLoadable.status === 'loading' && showEventTypesLoading) {
      return [{ value: '', label: 'Loading event types...' }];
    }
    return [];
  })();

  const [goToReview, setGoToReview] = useServerBackedField(existingTrigger?.go_to_review ?? false);
  const [codingAgentGrant, setCodingAgentGrant] = useServerBackedField(
    existingTrigger?.coding_agent_grant ?? false,
  );
  const [model, setModel] = useServerBackedField(existingTrigger?.model ?? '');
  const [triggerEffort, setTriggerEffort] = useServerBackedField(existingTrigger?.reasoning_effort ?? '');
  const [triggerProvider, setTriggerProvider] = useServerBackedField(existingTrigger?.provider ?? '');
  useEffect(() => {
    if (chatModels.value.status === 'not-loaded') void loadChatModels();
  }, []);

  const [groupId, setGroupId] = useServerBackedField(existingTrigger?.group_id ?? '');
  const [newGroupDraft, setNewGroupDraft] = useState<string | null>(null);
  const groupDraftSettledRef = useRef(false);

  const savingRef = useRef(false);
  const [saving, setSaving] = useState(false);

  const intentRef = useRef<HTMLTextAreaElement>(null);
  const resizeIntent = () => { if (intentRef.current) resizeTextarea(intentRef.current); };
  useEffect(resizeIntent, [intentText, runType]);
  useFontMetricsResize(resizeIntent);
  useWidthRemeasure(intentRef);

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
    if (savingRef.current) return;

    const run = buildRun();
    if (!run) return;

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

    savingRef.current = true;
    setSaving(true);
    try {
      await submitTrigger({
        name, run, cronExpressions: finalCrons, triggerId: editingId,
        on, showEvent, goToReview,
        groupId: groupId || null,
        codingAgentGrant,
        model: model || null,
        reasoningEffort: triggerEffort || null,
        provider: triggerProvider || null,
      });
    } finally {
      savingRef.current = false;
      setSaving(false);
    }
  }

  function clearSubError(index: number) {
    if (!subErrors[index]) return;
    const next = { ...subErrors };
    delete next[index];
    setSubErrors(next);
  }

  function addSubscription() {
    setSubscriptions({ subs: [...subs, { event_type: '' }], drafts: conditionDrafts });
  }

  function setEventType(index: number, eventType: string) {
    setSubscriptions({
      subs: subs.map((s, i) => i === index ? { ...s, event_type: eventType } : s),
      drafts: conditionDrafts,
    });
    clearSubError(index);
  }

  function setConditionDraft(index: number, source: string) {
    setSubscriptions({ subs, drafts: { ...conditionDrafts, [index]: source } });
    clearSubError(index);
  }

  function removeSubscription(index: number) {
    setSubscriptions({
      subs: subs.filter((_, i) => i !== index),
      drafts: reindexAfterRemoval(conditionDrafts, index),
    });
    setSubErrors(reindexAfterRemoval(subErrors, index));
  }

  function handleGroupChange(value: string) {
    if (value === NEW_GROUP_SENTINEL) {
      groupDraftSettledRef.current = false;
      setNewGroupDraft('');
      return;
    }
    setNewGroupDraft(null);
    setGroupId(value);
  }

  async function commitNewGroupDraft() {
    if (groupDraftSettledRef.current) return;
    const trimmed = (newGroupDraft ?? '').trim();
    if (!trimmed) { setNewGroupDraft(null); return; }
    groupDraftSettledRef.current = true;
    const group = await createTriggerGroup(trimmed);
    if (group) setGroupId(group.id);
    setNewGroupDraft(null);
  }

  const tierLabel = (value: string | null) => (value === null
    ? PROVIDER_DEFAULT_EFFORT_LABEL
    : REASONING_LEVELS.find(l => l.value === value)?.label ?? value);
  const accountModel = currentTriggerModel();
  const accountLabel =
    chatModelOptions().find(o => o.value === accountModel)?.label ?? accountModel;
  const inheritedTier = tierLabel(
    model === '' && triggerEffort ? triggerEffort : accountChatEffort(accountModel),
  );
  const modelChoices: ModelChoice[] = [
    { value: '', label: `Default (${accountLabel} · ${inheritedTier})`, reasoningEfforts: [] },
    ...lucidosModelChoices(model || null),
  ];

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
              {...PROSE_TEXT_ATTRS}
            />
          </div>

          <div class="form-group">
            <FieldLabel title="Group">
                <p>Optional folder shown in the triggers panel.</p>
                <p>
                  Group related triggers, e.g. steps of a workflow connected by{' '}
                  <code>emit_event</code> to <code>on_event</code>.
                </p>
            </FieldLabel>
            <Dropdown
              class="trigger-group-select"
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
                {...PROSE_TEXT_ATTRS}
                onInput={e => setNewGroupDraft((e.target as HTMLInputElement).value)}
                onBlur={commitNewGroupDraft}
                data-escape-self
                onKeyDown={e => {
                  if (e.key === 'Enter') { e.preventDefault(); void commitNewGroupDraft(); }
                  else if (e.key === 'Escape') { e.preventDefault(); groupDraftSettledRef.current = true; setNewGroupDraft(null); }
                }}
              />
            )}
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

              <div class={`input-with-action${cronError ? ' input-error' : ''}`}>
                <input
                  type="text"
                  value={cronInput}
                  onInput={(e) => {
                    setCronInput((e.target as HTMLInputElement).value);
                    setCronError(null);
                  }}
                  onKeyDown={handleCronKeyDown}
                  placeholder="0 0 8 * * *"
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
              <FieldLabel title="Event Subscriptions">
                  <p>
                    Each subscription's condition only filters that event: different
                    events can carry different payloads. A key is a field path, so
                    dots read into nested payload fields.
                  </p>
              </FieldLabel>
              {eventTypesLoadable.status === 'failed' && (
                <div class="form-error form-error-row">
                  <span>
                    Failed to load event types: {eventTypesLoadable.error}. You can
                    still type an event type by hand.
                  </span>
                  <button
                    type="button"
                    class="action-btn"
                    onClick={() => loadEventTypes(setEventTypesLoadable)}
                  >
                    Retry
                  </button>
                </div>
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
                      onOpen={() => retryFailedEventTypes(setEventTypesLoadable)}
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
                  {...PROSE_TEXT_ATTRS}
                />
              </div>
            </div>
          ) : (
            <div class="form-group">
              <FieldLabel title="Watching this trigger's runs" label="Script Path">
                {editingId && (
                  <>
                    <p>
                      Each run is recorded as events, and the trigger's row shows the
                      last run's OK/failed status.
                    </p>
                    <p>
                      For more on this trigger's runs (what it found, when, why a run
                      failed) ask the Lucidos Agent, e.g. “what has{' '}
                      {existingTrigger?.name || 'this trigger'} been finding?”, or
                      build an app on its events.
                    </p>
                  </>
                )}
              </FieldLabel>
              <input
                type="text"
                value={scriptPath}
                onInput={(e) => setScriptPath((e.target as HTMLInputElement).value)}
                placeholder="e.g. oura/run.py"
              />
            </div>
          )}

          {runType === 'intent' && (
            <>
              <div class="form-group">
                <FieldLabel title="Model">
                    <p>The model and reasoning this trigger's unattended runs use.</p>
                    <p>
                      Default follows Settings → Models → Chat &amp; triggers, so
                      pick here only when this trigger should differ, e.g.
                      something cheap for a routine digest or something stronger for a
                      weekly analysis.
                    </p>
                </FieldLabel>
                <ModelSelectionField
                  models={modelChoices}
                  vocabulary={LUCIDOS_TIER_VOCABULARY}
                  model={model}
                  effort={triggerEffort || null}
                  provider={triggerProvider || null}
                  onChange={(patch) => {
                    setModel(patch.model);
                    setTriggerEffort(patch.reasoningEffort ?? '');
                    setTriggerProvider(patch.provider ?? '');
                  }}
                />
              </div>

              <div class="form-group">
                <label class="form-checkbox-row">
                  <input
                    type="checkbox"
                    data-role="send-directly-to-archive"
                    checked={!goToReview}
                    onChange={(e) => setGoToReview(!(e.target as HTMLInputElement).checked)}
                  />
                  <span>Send directly to Archive</span>
                  <Explainer title="Send directly to Archive">
                    <p>On by default: each run goes to Archive when it finishes.</p>
                    <p>
                      Turn it off for triggers whose output you're meant to read:
                      daily summaries, alerts, scheduled reports. Their runs then
                      stay in Current.
                    </p>
                    <p>A run that asks you something stays in Current until you answer.</p>
                  </Explainer>
                </label>
              </div>

              <div class="form-group">
                <FieldLabel title="Program grants" />
                <div class="allowlist-rows">
                  {existingTrigger?.program_grants?.length
                    ? existingTrigger.program_grants.map((grant, index) => (
                      <AllowlistRow
                        key={`${grant.pattern}-${index}`}
                        pattern={grant.pattern}
                        detail={grant.migrated_invocation ? 'Migrated invocation' : undefined}
                        onDelete={editingId
                          ? () => void revokeTriggerGrant(editingId, { program: grant.pattern })
                          : undefined}
                      />
                    ))
                    : <div class="allowlist-empty">No program grants.</div>}
                </div>
              </div>
            </>
          )}

          <div class="form-group">
            <FieldLabel title="Folder grants" />
            <div class="allowlist-rows" data-role="folder-grants">
              {existingTrigger?.folder_grants?.length
                ? existingTrigger.folder_grants.map((grant) => (
                  <AllowlistRow
                    key={grant.folder}
                    pattern={grant.folder}
                    onDelete={editingId
                      ? () => void revokeTriggerGrant(editingId, { folder: grant.folder })
                      : undefined}
                  />
                ))
                : <div class="allowlist-empty">No folder grants.</div>}
            </div>
          </div>

          <div class="form-group">
            <FieldLabel title="Denied" />
            <div class="allowlist-rows" data-role="trigger-denies">
              {existingTrigger?.program_denies?.length || existingTrigger?.folder_denies?.length
                ? [
                  ...(existingTrigger.program_denies ?? []).map((program) => (
                    <AllowlistRow
                      key={`program-${program}`}
                      pattern={program}
                      detail="Program"
                      onDelete={editingId
                        ? () => void revokeTriggerDeny(editingId, { program })
                        : undefined}
                    />
                  )),
                  ...(existingTrigger.folder_denies ?? []).map((folder) => (
                    <AllowlistRow
                      key={`folder-${folder}`}
                      pattern={folder}
                      detail="Folder"
                      onDelete={editingId
                        ? () => void revokeTriggerDeny(editingId, { folder })
                        : undefined}
                    />
                  )),
                ]
                : <div class="allowlist-empty">Nothing denied.</div>}
            </div>
          </div>

          <div class="form-group">
            <label class="form-checkbox-row">
              <input
                type="checkbox"
                data-role="allow-coding-agents"
                checked={codingAgentGrant}
                onChange={(e) => setCodingAgentGrant((e.target as HTMLInputElement).checked)}
              />
              <span>Allow coding agents</span>
              <Explainer title="Allow coding agents">
                <p>
                  Off by default: a run that tries to start a coding agent fails, and
                  you get a notification.
                </p>
                <p>
                  Turn it on only if you trust what steers this trigger. Its coding
                  agents hold every grant you gave coding agents, so content the
                  trigger reads (messages, web pages, email) can steer them.
                </p>
                <p>Only you can turn this on. An agent or an app cannot.</p>
              </Explainer>
            </label>
          </div>

          <div class="form-actions">
            <button
              type="button"
              class="btn-cancel"
              onClick={closeTriggerForm}
            >
              Cancel
            </button>
            <button type="submit" class="btn-save" disabled={saving}>
              Save
            </button>
          </div>
        </div>
      </form>
    </div>
  );
}
