import { useEffect } from 'preact/hooks';
import { activeInlineForm, appsList } from '../../store/store';
import { closeAppForm, saveAppMetadata } from '../../store/actions/apps';
import type { App } from '../../store/types';
import { AutoTextarea } from '../shared/AutoTextarea';
import { LoadableError } from '../shared/LoadableError';
import { FormSkeleton } from '../shared/FormSkeleton';
import { LoadingFade } from '../shared/LoadingFade';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { useServerBackedField } from '../../hooks/useServerBackedField';

export function AppUiEditModal() {
  const form = activeInlineForm.value;
  const showLoading = useDelayedLoading(appsList.value);
  if (form?.type !== 'app-edit') return null;

  const { appId } = form;

  if (appsList.value.status === 'failed') {
    return (
      <div class="inline-form">
        <LoadableError error={appsList.value.error} noun="apps" />
      </div>
    );
  }
  const apps = appsList.value.status === 'loaded' ? appsList.value.data : null;
  const app = apps?.find((s) => s.id === appId);
  if (apps && !app) return <MissingAppCloser key={appId} />;

  return (
    <LoadingFade showSkeleton={showLoading} skeleton={<FormSkeleton fields={APP_EDIT_FORM_FIELDS} />}>
      {app && <AppUiEditModalInner key={appId} app={app} />}
    </LoadingFade>
  );
}

const APP_EDIT_FORM_FIELDS = [{ label: 'Name' }, { label: 'Description' }];

function MissingAppCloser() {
  useEffect(() => { closeAppForm(); }, []);
  return null;
}

function AppUiEditModalInner({ app }: { app: App }) {
  const [name, setName] = useServerBackedField(app.name);
  const [description, setDescription] = useServerBackedField(app.description);

  async function handleSave(e: Event) {
    e.preventDefault();
    if (!name.trim()) return;
    if (await saveAppMetadata(app.id, name.trim(), description.trim())) closeAppForm();
  }

  return (
    <div class="inline-form">
      <form onSubmit={handleSave}>
        <div class="inline-form-body">
          <div class="form-group">
            <label>Name</label>
            <input
              type="text"
              value={name}
              onInput={(e) => setName((e.target as HTMLInputElement).value)}
              required
            />
          </div>
          <div class="form-group">
            <label>Description</label>
            <AutoTextarea value={description} onInput={setDescription} />
          </div>
          <div class="form-actions">
            <button type="button" class="btn-cancel" onClick={closeAppForm}>
              Cancel
            </button>
            <button type="submit" class="btn-save">Save</button>
          </div>
        </div>
      </form>
    </div>
  );
}
