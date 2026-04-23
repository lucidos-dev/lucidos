import { useState, useEffect } from 'preact/hooks';
import { activeInlineForm, triggers, showToast } from '../../store/store';
import {
  closeTriggerForm,
  submitTrigger,
} from '../../store/actions/triggers';
import { deriveTriggerType } from '../../store/types';
import type { TriggerInfo, TriggerRun } from '../../store/types';
import { describeCron, validateCron } from '../../utils/describeCron';
import { Dropdown } from '../shared/Dropdown';
import { fetchEventTypes } from '../../api/client';

type TriggerFormType = 'schedule' | 'event' | 'both';
type RunType = 'intent' | 'script';

let cachedEventTypes: string[] | null = null;

export function TriggerModal() {
  const form = activeInlineForm.value;
  if (form?.type !== 'trigger') return null;

  const editingId = form.taskId;

  if (editingId) {
    if (triggers.value.status !== 'loaded') {
      return <div class="inline-form"><div class="empty">Loading...</div></div>;
    }
    const task = triggers.value.data.find((t) => t.id === editingId);
    if (!task) {
      closeTriggerForm();
      return null;
    }
    return <TriggerFormInner key={editingId} editingId={editingId} existingTask={task} />;
  }

  return <TriggerFormInner key="new" existingTask={null} />;
}

function TriggerFormInner({ editingId, existingTask }: { editingId?: string; existingTask: TriggerInfo | null }) {

  const derived = existingTask ? deriveTriggerType(existingTask) : null;
  const initialFormType: TriggerFormType = derived === 'hybrid' ? 'both' : derived === 'event' ? 'event' : 'schedule';

  const [formType, setFormType] = useState<TriggerFormType>(initialFormType);
  const [name, setName] = useState(existingTask?.name || '');

  const existingRun = existingTask?.run;
  const [runType, setRunType] = useState<RunType>(existingRun?.type === 'script' ? 'script' : 'intent');
  const [intentText, setIntentText] = useState(
    existingRun?.type === 'intent' ? existingRun.text : ''
  );
  const [scriptPath, setScriptPath] = useState(
    existingRun?.type === 'script' ? existingRun.path : ''
  );

  const [cronExpressions, setCronExpressions] = useState<string[]>(
    existingTask?.cron_expressions?.length ? [...existingTask.cron_expressions] : []
  );
  const [cronInput, setCronInput] = useState('');
  const [cronError, setCronError] = useState<string | null>(null);
  const [eventType, setEventType] = useState(existingTask?.on || '');
  const [knownEventTypes, setKnownEventTypes] = useState<string[]>(cachedEventTypes ?? []);

  useEffect(() => {
    if (cachedEventTypes) return;
    fetchEventTypes()
      .then(types => { cachedEventTypes = types; setKnownEventTypes(types); })
      .catch(() => showToast('Failed to load event types', 'error'));
  }, []);

  const [conditionJson, setConditionJson] = useState(
    existingTask?.condition ? JSON.stringify(existingTask.condition, null, 2) : ''
  );

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
      const knowhow = existingRun?.type === 'intent' ? existingRun.knowhow : [];
      return { type: 'intent', text: intentText.trim(), knowhow };
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

    // Parse condition JSON if provided
    let condition: Record<string, unknown> | undefined;
    if (showEvent && conditionJson.trim()) {
      try {
        condition = JSON.parse(conditionJson.trim());
      } catch {
        showToast('Invalid JSON in condition field', 'error');
        return;
      }
    }

    const onEvent = showEvent && eventType.trim() ? eventType.trim() : undefined;

    await submitTrigger({
      name, run, cronExpressions: finalCrons, taskId: editingId,
      onEvent, condition, showEvent,
    });
  }

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
                <ul class="cron-list">
                  {cronExpressions.map((expr, i) => (
                    <li key={i} class="cron-list-item">
                      <div class="cron-list-item-info">
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

              <div class="cron-input-row">
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
            <>
              <div class="form-group">
                <label>Event Type</label>
                <Dropdown
                  options={knownEventTypes.map(t => ({ value: t, label: t }))}
                  value={eventType}
                  onChange={setEventType}
                  placeholder="e.g. OuraSleepImported"
                  freeText
                />
              </div>

              <div class="form-group">
                <label>Condition (optional)</label>
                <textarea
                  value={conditionJson}
                  onInput={(e) => setConditionJson((e.target as HTMLTextAreaElement).value)}
                  placeholder={'e.g. {"sleep_score": {"$lt": 70}}'}
                  class="code-textarea"
                  rows={3}
                />
                <div class="form-hint">JSON payload filter. Leave empty to fire on every matching event.</div>
              </div>
            </>
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
              <textarea
                value={intentText}
                onInput={(e) => setIntentText((e.target as HTMLTextAreaElement).value)}
                placeholder="e.g. Check my calendar and send me a summary of today's events"
                rows={3}
              />
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
