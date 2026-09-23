import { Fragment } from 'preact';
import { useState } from 'preact/hooks';
import { chatModels } from '../../store/store';
import { Dropdown } from '../shared/Dropdown';
import { LoadableError } from '../shared/LoadableError';
import { ListRowAddCard } from '../shared/ListRowAddCard';
import { ChevronDownIcon, ChevronRightIcon } from '../shared/icons';
import {
  setModelEnabled, deleteModel, submitNewModel, hasConfiguredRoute, resolvedRoute,
} from '../../store/actions/models';
import { formatContextWindow } from '../../utils/formatTokens';
import { ModelRoutesEditor } from './ModelRoutesEditor';
import type { ModelInfo } from '../../api/types';
import { PROVIDERS } from '../../store/models';

/** The route that will serve this model, which is what the row's window
 *  describes. A window is per backend, so naming the row's first would be wrong
 *  the moment the preferred one declares a different one. */
function served(m: ModelInfo) {
  return resolvedRoute(m.id) ?? m.routes[0];
}

/** Which row's route editor is open, and how to open or close one. */
export interface RouteEditing {
  id: string | null;
  open: (id: string | null) => void;
}

/** One registry row: the switch, the model's name and id, then the metadata
 *  cluster, and the route editor under it when open. Shared by the on list and
 *  the collapsed Off group, so a switched-off model keeps its switch and a
 *  switched-off user model keeps its Delete. */
function modelRow(m: ModelInfo, editing?: RouteEditing) {
  const editorOpen = editing?.id === m.id;
  return (
    <Fragment key={m.id}>
      <div class="model-manager-row">
        <label class="toggle-switch">
          <input
            type="checkbox"
            checked={m.enabled}
            aria-label={`Offer ${m.label} in the model picker`}
            onChange={(e) =>
              void setModelEnabled(m.id, (e.currentTarget as HTMLInputElement).checked)
            }
          />
          <span class="toggle-slider" />
        </label>
        <div class="model-manager-info">
          <div class="model-manager-name">{m.label}</div>
          <div class="model-manager-id">
            {m.id} · {formatContextWindow(served(m)?.context_window ?? null)}
          </div>
        </div>
        <div class="model-manager-meta">
          {/* One badge per backend that can serve this model, in the order the
              engine resolves them. The preferred one is marked, since a pick is
              honoured or refused rather than substituted: reading which backend
              a turn will reach is the point of the row. */}
          {m.routes.map((r) => (
            <span
              key={r.provider}
              class="model-provider-badge"
              data-preferred={r.provider === m.preferred_provider ? 'true' : undefined}
              data-tooltip={r.id === m.id ? undefined : `Sent as ${r.id}`}
            >
              {r.provider}
            </span>
          ))}
          {!hasConfiguredRoute(m) && (
            <span
              class="model-provider-badge model-provider-unconfigured"
              data-tooltip="No provider set up. This model stays out of the picker until you add one under Providers"
            >
              not set up
            </span>
          )}
          {editing && (
            <button
              class="action-btn"
              aria-expanded={editorOpen}
              onClick={() => editing.open(editorOpen ? null : m.id)}
            >
              Routes
            </button>
          )}
          {m.source === 'user' ? (
            <button class="action-btn action-btn-danger" onClick={() => void deleteModel(m.id)}>
              Delete
            </button>
          ) : (
            <span class="list-row-details">builtin</span>
          )}
        </div>
      </div>
      {editorOpen && <ModelRoutesEditor model={m} onClose={() => editing.open(null)} />}
    </Fragment>
  );
}

/** The registry, split into what the picker offers and what it does not.
 *
 *  The off rows sit behind a disclosure because a retired builtin keeps its row
 *  forever: disable-only is the whole reason routing still resolves a saved
 *  `chat_model`. Left flat, the page is mostly models the picker will never
 *  offer. They stay one tap away, with their switch and their Delete intact.
 *
 *  A pure function, so the grouping is testable without a DOM. The component
 *  owns the state; this owns the markup. */
export function modelManagerList(
  models: readonly ModelInfo[],
  showOff: boolean,
  onToggleOff: () => void,
  editing?: RouteEditing,
) {
  const on = models.filter((m) => m.enabled);
  const off = models.filter((m) => !m.enabled);
  return (
    <>
      {on.map((m) => modelRow(m, editing))}
      {off.length > 0 && (
        <>
          <button
            type="button"
            class="settings-disclosure-toggle"
            aria-expanded={showOff}
            onClick={onToggleOff}
          >
            {showOff ? <ChevronDownIcon size="1rem" /> : <ChevronRightIcon size="1rem" />}
            Off ({off.length})
          </button>
          {showOff && off.map((m) => modelRow(m, editing))}
        </>
      )}
    </>
  );
}

/** Settings → Models manager: list every registry model with a provider badge
 *  and enable toggle (builtins are disable-only; user models can be deleted),
 *  plus an inline "Add Model" form. Drives the DB-backed registry via the model
 *  actions; the chat picker re-reads it on the Model* SSE. */
export function ModelsManager() {
  const loadable = chatModels.value;
  const [adding, setAdding] = useState(false);
  const [showOff, setShowOff] = useState(false);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [id, setId] = useState('');
  const [label, setLabel] = useState('');
  const [provider, setProvider] = useState('anthropic');
  const [contextWindow, setContextWindow] = useState('');

  async function add() {
    const ok = await submitNewModel(id, label, provider, contextWindow);
    if (ok) {
      setId('');
      setLabel('');
      setProvider('anthropic');
      setContextWindow('');
      setAdding(false);
    }
  }

  return (
    <div class="settings-section">
      <div class="settings-section-title" data-search-anchor="models:manage">Manage models</div>
      {loadable.status === 'failed' && (
        <LoadableError noun="models" error={loadable.error} />
      )}
      {loadable.status === 'loaded' && (
        <>
          {modelManagerList(loadable.data, showOff, () => setShowOff((prev) => !prev), {
            id: editingId,
            open: setEditingId,
          })}
          {adding ? (
            <div class="settings-section">
              <div class="settings-row">
                <span class="settings-row-label">Model id</span>
                <input
                  class="settings-text-input"
                  placeholder="claude-fable-5"
                  value={id}
                  onInput={(e) => setId((e.currentTarget as HTMLInputElement).value)}
                />
              </div>
              <div class="settings-row">
                <span class="settings-row-label">Label</span>
                <input
                  class="settings-text-input"
                  placeholder="Fable 5"
                  value={label}
                  onInput={(e) => setLabel((e.currentTarget as HTMLInputElement).value)}
                />
              </div>
              <div class="settings-row">
                <span class="settings-row-label">Provider</span>
                <Dropdown options={PROVIDERS} value={provider} onChange={setProvider} />
              </div>
              <div class="settings-row">
                <span class="settings-row-label">Context window</span>
                <input
                  class="settings-text-input"
                  inputMode="numeric"
                  placeholder="1048576 — leave blank to infer from the id"
                  value={contextWindow}
                  onInput={(e) =>
                    setContextWindow((e.currentTarget as HTMLInputElement).value)
                  }
                />
              </div>
              <div class="settings-row">
                <span class="settings-row-label" />
                {/* A sentence, not a row of fields, so it takes the prose
                    modifier: the base class is a flex row whose 0.75rem gap is
                    the field separator (`.claude/rules/frontend.md`). */}
                <span class="list-row-details list-row-details-prose">
                  Tokens. Left blank, the engine guesses from the model id — and it
                  has no rule for OpenRouter, xAI, Gemini, or local models, so they are
                  treated as 200k however large they really are.
                </span>
              </div>
              <div class="settings-row">
                <span class="settings-row-label" />
                <div class="settings-row-options">
                  <button class="action-btn" onClick={() => setAdding(false)}>
                    Cancel
                  </button>
                  <button
                    class="action-btn action-btn-confirm"
                    disabled={!id.trim() || !label.trim()}
                    onClick={() => void add()}
                  >
                    Add
                  </button>
                </div>
              </div>
            </div>
          ) : (
            <ListRowAddCard label="Add Model" onClick={() => setAdding(true)} />
          )}
        </>
      )}
    </div>
  );
}
