import { useState, useEffect, useRef } from 'preact/hooks';
import { activeInlineForm, triggers, triggerGroups, showToast, chatModels, currentModel, reasoningEffort } from '../../store/store';
import {
  closeTriggerForm,
  submitTrigger,
} from '../../store/actions/triggers';
import {
  chatModelOptions, loadChatModels, lucidosModelChoices, LUCIDOS_TIER_VOCABULARY,
} from '../../store/actions/models';
import type { ModelChoice } from '../../store/modelSelection';
import { ModelSelectionField } from '../shared/ModelSelectionField';
import { REASONING_LEVELS } from '../../store/models';
import { createTriggerGroup } from '../../store/actions/triggerGroups';
import { deriveTriggerType, toFailed } from '../../store/types';
import type { EventSubscription, SideEffectCategory, TriggerInfo, TriggerRun, Loadable } from '../../store/types';
import { describeCron, validateCron } from '../../utils/describeCron';
import { Dropdown } from '../shared/Dropdown';
import { fetchEventTypes } from '../../api/client';
import { resizeTextarea, useFontMetricsResize } from '../chat/promptResize';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { useServerBackedField, sameJson, sameSet } from '../../hooks/useServerBackedField';
import { LoadableError } from '../shared/LoadableError';
import { Explainer, FieldLabel } from '../shared/Explainer';
import { PROSE_TEXT_ATTRS } from '../../utils/noAutofill';

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
//
// A failure is not sticky. `loadEventTypes` runs unless the cache is `loaded`.
// So the next mount retries, the error row's Retry button re-drives it, and
// opening the picker retries it too. A loaded list is never refreshed for the
// life of the page. An event type first emitted after that load is missing
// until Retry, or a reload.
let cachedEventTypes: Loadable<string[]> = { status: 'not-loaded' };
let inflightFetch: Promise<string[]> | null = null;

/** Fill the module cache, deduping concurrent callers onto one request.
 *
 *  `apply` publishes each state to the calling component. Every retry path
 *  comes through here, so there is one load path. */
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

/** Retry a failed list from the user's own gesture of opening the picker, the
 *  same bargain `retryFailedDestinationLists` strikes for the compose
 *  destination lists. The error row's Retry button is the visible affordance;
 *  this catches the user who opens the picker without reading the row. Only a
 *  failed cache refetches, and `loadEventTypes` single-flights, so an open
 *  costs at most one request. */
function retryFailedEventTypes(apply: (l: Loadable<string[]>) => void): void {
  if (cachedEventTypes.status === 'failed') loadEventTypes(apply);
}

/** The event-subscription editor's value. The parsed condition lives on
 *  `subs[i].condition` so submit can forward it directly. The raw JSON the
 *  user is typing lives in `drafts[i]`. Parsing it back on every keystroke
 *  would lose the in-flight cursor and every invalid intermediate state. Both
 *  are keyed by row index, so they travel as one value. */
interface SubscriptionDraft {
  subs: EventSubscription[];
  drafts: Record<number, string>;
}

/** The subscription editor as the stored trigger describes it. */
function servedSubscriptions(on: EventSubscription[] | undefined): SubscriptionDraft {
  const subs = (on ?? []).map(s => ({ ...s }));
  const drafts: Record<number, string> = {};
  subs.forEach((s, i) => {
    if (s.condition) drafts[i] = JSON.stringify(s.condition, null, 2);
  });
  return { subs, drafts };
}

/** Rekey an index-keyed map after row `index` was removed, so trailing entries
 *  shift down with the array they annotate. */
function reindexAfterRemoval<T>(map: Record<number, T>, index: number): Record<number, T> {
  return Object.fromEntries(
    Object.entries(map)
      .map(([k, v]) => [Number(k), v] as const)
      .filter(([k]) => k !== index)
      .map(([k, v]) => [k > index ? k - 1 : k, v]),
  );
}

export function TriggerDetails() {
  const form = activeInlineForm.value;
  // Delay the spinner (300ms) so a fast load never flashes it.
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
    if (showLoading) {
      return <div class="inline-form"><div class="loading-spinner" /></div>;
    }
    if (triggers.value.status !== 'loaded') {
      // Pre-delay window — keep the container mounted but show nothing yet.
      return <div class="inline-form" />;
    }
    const trigger = triggers.value.data.find((t) => t.id === editingId);
    if (!trigger) {
      // key={editingId} forces a remount when activeInlineForm flips from
      // missing-A to missing-B in successive renders: without it, Preact reuses
      // the instance and the empty-deps useEffect never re-fires for B, leaving
      // the form open on a trigger that no longer exists. Same guard as
      // AppUiEditModal's MissingAppCloser.
      return <MissingTriggerCloser key={editingId} />;
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

  // Every field below is a *server-backed field*: untouched it renders the
  // trigger the store holds, so a `TriggerUpdated` frame repaints the open
  // page; touched it holds the user's draft. See ADR 0118.
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

  // One field, not two: the rows and their drafts are index-parallel, and
  // `removeSubscription` reindexes both. Touching one alone would desync them.
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
    // A failed load offers nothing. An empty-valued placeholder would clear
    // the field, and `freeText` keeps hand-entry working with no options.
    return [];
  })();

  const [goToReview, setGoToReview] = useServerBackedField(existingTrigger?.go_to_review ?? false);
  const [sideEffectGrant, setSideEffectGrant] = useServerBackedField<SideEffectCategory[]>(
    existingTrigger?.side_effect_grant ?? [], sameSet,
  );
  const toggleSideEffect = (cat: SideEffectCategory, on: boolean) => {
    setSideEffectGrant(
      on ? [...sideEffectGrant.filter(c => c !== cat), cat] : sideEffectGrant.filter(c => c !== cat)
    );
  };
  // '' is the Default option: the trigger inherits the account chat model /
  // effort, which is what a trigger without a pin has always done. Kept as ''
  // rather than null so the <Dropdown> value round-trips as a plain string.
  const [model, setModel] = useServerBackedField(existingTrigger?.model ?? '');
  const [triggerEffort, setTriggerEffort] = useServerBackedField(existingTrigger?.reasoning_effort ?? '');
  // The picker reads the DB-backed registry; kick a load if nothing has yet
  // (loadChatModels single-flights via setLoadingIfFresh). Until it lands,
  // chatModelOptions() falls back to the static list, so the field is never
  // empty. Same guard as the in-thread Lucidos control menu.
  useEffect(() => {
    if (chatModels.value.status === 'not-loaded') void loadChatModels();
  }, []);

  const [groupId, setGroupId] = useServerBackedField(existingTrigger?.group_id ?? '');
  // null = inline-create field hidden; string = visible with current draft.
  const [newGroupDraft, setNewGroupDraft] = useState<string | null>(null);
  // Enter and blur BOTH call commitNewGroupDraft; committing hides the field,
  // whose trailing blur would POST the same name again (409). Submit-once flag,
  // reset when the inline-create field reopens.
  const creatingGroupRef = useRef(false);

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

    // go_to_review / side_effect_grant / model / reasoning_effort apply only to
    // the intent path; `submitTrigger` gates all four on `run.type` so a script
    // trigger can't persist state left over from an intent → script switch.
    await submitTrigger({
      name, run, cronExpressions: finalCrons, triggerId: editingId,
      on, showEvent, goToReview,
      groupId: groupId || null,
      sideEffectGrant,
      // '' is the Default option; null is how it reaches the engine.
      model: model || null,
      reasoningEffort: triggerEffort || null,
    });
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
      creatingGroupRef.current = false;
      setNewGroupDraft('');
      return;
    }
    setNewGroupDraft(null);
    setGroupId(value);
  }

  async function commitNewGroupDraft() {
    if (creatingGroupRef.current) return;
    const trimmed = (newGroupDraft ?? '').trim();
    if (!trimmed) { setNewGroupDraft(null); return; }
    creatingGroupRef.current = true;
    const group = await createTriggerGroup(trimmed);
    if (group) setGroupId(group.id);
    setNewGroupDraft(null);
  }

  // The trigger form uses the shared dropdown shell inside its own
  // `.form-group`, because the field explainer lives here.
  //
  // Its extra row is Default, which inherits the whole pair. It carries no
  // tiers of its own, so picking it clears both halves. Its label names the
  // pair the run will actually use. For a legacy trigger that pinned an effort
  // and no model, that is the pinned tier.
  const tierLabel = (value: string) =>
    REASONING_LEVELS.find(l => l.value === value)?.label ?? value;
  const accountLabel =
    chatModelOptions().find(o => o.value === currentModel.value)?.label ?? currentModel.value;
  // The account's tier, except on a legacy trigger that pinned an effort and no
  // model. Once a model IS pinned the tier belongs to that pick, and naming it
  // here would claim Default runs on it.
  const inheritedTier = tierLabel(
    model === '' && triggerEffort ? triggerEffort : reasoningEffort.value,
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
                onKeyDown={e => {
                  if (e.key === 'Enter') { e.preventDefault(); void commitNewGroupDraft(); }
                  else if (e.key === 'Escape') setNewGroupDraft(null);
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
              {/* Script triggers are threadless (there is no run thread to
                  open), so point at how to observe them instead. Only when
                  editing an existing trigger: a brand-new one has no runs yet,
                  and a falsy child leaves FieldLabel a plain label. */}
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

          {/* Both controls are consumed only on the intent (LLM) trigger path;
              the script path never reads go_to_review or side_effect_grant. Hide
              them in script mode so they can't imply behavior that never runs. */}
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
                  onChange={(patch) => {
                    setModel(patch.model);
                    setTriggerEffort(patch.reasoningEffort ?? '');
                  }}
                />
              </div>

              <div class="form-group">
                <label class="form-checkbox-row">
                  <input
                    type="checkbox"
                    checked={goToReview}
                    onChange={(e) => setGoToReview((e.target as HTMLInputElement).checked)}
                  />
                  <span>Send to Review on completion</span>
                  <Explainer title="Send to Review on completion">
                    <p>By default, runs land in Archive.</p>
                    <p>
                      Turn this on for triggers whose output you're meant to read:
                      daily summaries, alerts, scheduled reports.
                    </p>
                  </Explainer>
                </label>
              </div>

              <div class="form-group">
                <FieldLabel title="Allowed side-effects">
                    <p>Only used when command safety is on (Settings → Permissions).</p>
                    <p>
                      This trigger runs unattended, so it can't be asked to approve a
                      risky command. Grant only the irreversible side-effects its intent
                      genuinely needs: anything else is blocked and the run fails.
                    </p>
                    <p>
                      Leave all off if it only reads, computes, or writes inside the
                      workspace.
                    </p>
                </FieldLabel>
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
              </div>
            </>
          )}

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
