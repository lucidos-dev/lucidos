import { useEffect, useState } from 'preact/hooks';
import { environmentVariables } from '../../store/store';
import {
  loadEnvironmentVariables,
  submitEnvVar,
  deleteEnvironmentVariable,
} from '../../store/actions/environmentVariables';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { DelayedSpinner } from '../shared/DelayedSpinner';
import { LoadableError } from '../shared/LoadableError';
import type { EnvironmentVariable } from '../../api/types';

const NAME_HINT = 'Uppercase letters, digits and underscores; must start with a letter or underscore (e.g. LUCIDOS_REPO). Reserved names are rejected by the server.';

/** Inline add/edit form row, mirroring AddRepositoryForm. */
function AddEnvVarForm() {
  const [adding, setAdding] = useState(false);
  const [name, setName] = useState('');
  const [value, setValue] = useState('');
  const [saving, setSaving] = useState(false);

  if (!adding) {
    return (
      <div class="list-row-add-card" onClick={() => setAdding(true)}>
        <div class="list-row-add-icon">+</div>
        <div class="list-row-add-label">Add Environment Variable</div>
      </div>
    );
  }

  function reset() {
    setAdding(false);
    setName('');
    setValue('');
  }

  async function save() {
    if (!name.trim()) return;
    setSaving(true);
    try {
      const ok = await submitEnvVar(name, value, false);
      if (ok) reset();
    } finally {
      setSaving(false);
    }
  }

  return (
    <div class="list-row repo-add-form">
      <div class="list-row-info" style={{ gap: '0.5rem' }}>
        <input
          class="device-name-input"
          type="text"
          placeholder="Name (e.g. LUCIDOS_REPO)"
          value={name}
          onInput={(e) => setName((e.target as HTMLInputElement).value)}
          onKeyDown={(e) => { if (e.key === 'Enter') void save(); if (e.key === 'Escape') reset(); }}
          autoFocus
        />
        <input
          class="device-name-input"
          type="text"
          placeholder="Value"
          value={value}
          onInput={(e) => setValue((e.target as HTMLInputElement).value)}
          onKeyDown={(e) => { if (e.key === 'Enter') void save(); if (e.key === 'Escape') reset(); }}
        />
        <div class="settings-section-desc" style={{ margin: 0 }}>{NAME_HINT}</div>
      </div>
      <div class="list-row-actions">
        <button class="action-btn action-btn-confirm" disabled={saving || !name.trim()} onClick={save}>
          {saving ? 'Saving...' : 'Save'}
        </button>
        <button class="action-btn" onClick={reset}>Cancel</button>
      </div>
    </div>
  );
}

/** A single env var row with inline edit, mirroring DeviceRow / repositoriesSection. */
function EnvVarRow({ envVar }: { envVar: EnvironmentVariable }) {
  const [editing, setEditing] = useState(false);
  const [value, setValue] = useState(envVar.value);
  const [saving, setSaving] = useState(false);

  function startEditing() {
    setValue(envVar.value);
    setEditing(true);
  }

  async function saveEdit() {
    setSaving(true);
    try {
      const ok = await submitEnvVar(envVar.name, value, true);
      if (ok) setEditing(false);
    } finally {
      setSaving(false);
    }
  }

  if (editing) {
    return (
      <div class="list-row">
        <div class="list-row-info" style={{ gap: '0.5rem' }}>
          <div class="title">{envVar.name}</div>
          <input
            class="device-name-input"
            type="text"
            value={value}
            onInput={(e) => setValue((e.target as HTMLInputElement).value)}
            onKeyDown={(e) => { if (e.key === 'Enter') void saveEdit(); if (e.key === 'Escape') setEditing(false); }}
            autoFocus
          />
        </div>
        <div class="list-row-actions">
          <button class="action-btn action-btn-confirm" disabled={saving} onClick={saveEdit}>
            {saving ? 'Saving...' : 'Save'}
          </button>
          <button class="action-btn" onClick={() => setEditing(false)}>Cancel</button>
        </div>
      </div>
    );
  }

  return (
    <div class="list-row">
      <div class="list-row-info">
        <div class="title">{envVar.name}</div>
        <div class="list-row-details">{envVar.value}</div>
      </div>
      <div class="list-row-actions">
        <button class="action-btn" onClick={startEditing}>Edit</button>
        <button class="action-btn action-btn-danger" onClick={() => void deleteEnvironmentVariable(envVar.name)}>
          Delete
        </button>
      </div>
    </div>
  );
}

export function EnvironmentVariablesPage() {
  const loadable = environmentVariables.value;
  const showLoading = useDelayedLoading(loadable);

  useEffect(() => {
    if (environmentVariables.value.status === 'not-loaded') {
      void loadEnvironmentVariables();
    }
  }, []);

  if (loadable.status === 'failed') {
    return (
      <div class="settings-section">
        <div class="list-rows"><LoadableError noun="environment variables" error={loadable.error} /></div>
      </div>
    );
  }
  if (loadable.status !== 'loaded') {
    if (!showLoading) return null;
    return (
      <div class="settings-section">
        <DelayedSpinner />
      </div>
    );
  }

  return (
    <div class="settings-section">
      <p class="settings-section-desc">
        User-managed environment variables made available to agent scripts and tools.
      </p>
      <div class="list-rows">
        {loadable.data.length === 0 ? (
          <div class="empty-state">No environment variables yet.</div>
        ) : (
          loadable.data.map((envVar) => <EnvVarRow key={envVar.id} envVar={envVar} />)
        )}
        <AddEnvVarForm />
      </div>
    </div>
  );
}
