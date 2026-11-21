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
  // Delay the spinner (300ms) so a fast load never flashes it.
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
  // key={appId} remounts the closer when the form flips from missing-A to
  // missing-B. Without it, Preact reuses the instance, and the empty-deps
  // useEffect never re-fires for B.
  if (apps && !app) return <MissingAppCloser key={appId} />;

  return (
    <LoadingFade showSkeleton={showLoading} skeleton={<FormSkeleton fields={APP_EDIT_FORM_FIELDS} />}>
      {app && <AppUiEditModalInner key={appId} app={app} />}
    </LoadingFade>
  );
}

const APP_EDIT_FORM_FIELDS = [{ label: 'Name' }, { label: 'Description' }];

/** Closes the app-edit form when the target app no longer exists. Lives in a
 *  child component so the signal write happens in useEffect (post-commit),
 *  not inside the parent's render body. */
function MissingAppCloser() {
  useEffect(() => { closeAppForm(); }, []);
  return null;
}

function AppUiEditModalInner({ app }: { app: App }) {
  // Server-backed: an `AppUpdated` frame repaints an untouched field, and a
  // touched one keeps the user's draft (ADR 0118).
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
