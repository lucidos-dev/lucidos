import { useState } from 'preact/hooks';
import { chatModels } from '../../store/store';
import { Dropdown } from '../shared/Dropdown';
import { LoadableError } from '../shared/LoadableError';
import { setModelEnabled, deleteModel, submitNewModel, isProviderConfigured } from '../../store/actions/models';

const PROVIDERS = [
  { value: 'anthropic', label: 'Anthropic (direct)' },
  { value: 'vertex', label: 'Vertex' },
  { value: 'openai', label: 'OpenAI' },
  { value: 'openrouter', label: 'OpenRouter' },
  { value: 'local', label: 'Local (OpenAI-compatible)' },
];

/** Settings → Models manager: list every registry model with a provider badge
 *  and enable toggle (builtins are disable-only; user models can be deleted),
 *  plus an inline "Add Model" form. Drives the DB-backed registry via the model
 *  actions; the chat picker re-reads it on the Model* SSE. */
export function ModelsManager() {
  const loadable = chatModels.value;
  const [adding, setAdding] = useState(false);
  const [id, setId] = useState('');
  const [label, setLabel] = useState('');
  const [provider, setProvider] = useState('anthropic');

  async function add() {
    const ok = await submitNewModel(id, label, provider);
    if (ok) {
      setId('');
      setLabel('');
      setProvider('anthropic');
      setAdding(false);
    }
  }

  return (
    <div class="settings-section">
      <div class="settings-section-title" data-search-anchor="models:manage">Manage Models</div>
      {loadable.status === 'failed' && (
        <LoadableError noun="models" error={loadable.error} />
      )}
      {loadable.status === 'loaded' && (
        <>
          {loadable.data.map((m) => (
            <div class="model-manager-row" key={m.id}>
              <label class="toggle-switch">
                <input
                  type="checkbox"
                  checked={m.enabled}
                  onChange={(e) =>
                    void setModelEnabled(m.id, (e.currentTarget as HTMLInputElement).checked)
                  }
                />
                <span class="toggle-slider" />
              </label>
              <div class="model-manager-info">
                <div class="model-manager-name">{m.label}</div>
                <div class="model-manager-id">{m.id}</div>
              </div>
              <span class="model-provider-badge">{m.provider}</span>
              {!isProviderConfigured(m.provider) && (
                <span
                  class="model-provider-badge model-provider-unconfigured"
                  data-tooltip="Provider not set up — this model is hidden from the picker until you add it under Providers"
                >
                  not set up
                </span>
              )}
              {m.source === 'user' ? (
                <button class="action-btn action-btn-danger" onClick={() => void deleteModel(m.id)}>
                  Delete
                </button>
              ) : (
                <span class="list-row-details">builtin</span>
              )}
            </div>
          ))}
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
            <div class="list-row-add-card" onClick={() => setAdding(true)}>
              <div class="list-row-add-icon">+</div>
              <div class="list-row-add-label">Add Model</div>
            </div>
          )}
        </>
      )}
    </div>
  );
}
