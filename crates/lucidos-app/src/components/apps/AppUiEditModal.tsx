import { useState, useEffect, useRef } from 'preact/hooks';
import { activeInlineForm, appsList, showToast } from '../../store/store';
import { closeAppForm, saveAppMetadata, refreshAppUI } from '../../store/actions/apps';
// Preact lint flags signal writes from a render body as a side-effect-in-render
// bug; the missing-app close is moved into a useEffect via MissingAppCloser
// instead of calling closeAppForm() inline.
import type { App, Loadable } from '../../store/types';
import { readAppSourceApi, writeAppSourceApi } from '../../api/client';
import type { UiSourceFile } from '../../api/client';
import { AutoTextarea } from '../shared/AutoTextarea';
import { autoResizeTextarea } from '../../utils/dom';
import { useLoadableFetch } from '../../hooks/useLoadableFetch';
import { errorDetail } from '../../utils/errorDetail';
import { LoadableError } from '../shared/LoadableError';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';

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
  if (showLoading) {
    return <div class="inline-form"><div class="loading-spinner" /></div>;
  }
  if (appsList.value.status !== 'loaded') {
    // Pre-delay window — keep the container mounted but show nothing yet.
    return <div class="inline-form" />;
  }

  const app = appsList.value.data.find((s) => s.id === appId);
  if (!app) {
    // key={appId} forces a remount when activeInlineForm flips from missing-A
    // to missing-B in successive renders — without it, Preact reuses the
    // instance and the empty-deps useEffect never re-fires for B.
    return <MissingAppCloser key={appId} />;
  }

  return <AppUiEditModalInner key={appId} app={app} />;
}

/** Closes the app-edit form when the target app no longer exists. Lives in a
 *  child component so the signal write happens in useEffect (post-commit),
 *  not inside the parent's render body. */
function MissingAppCloser() {
  useEffect(() => { closeAppForm(); }, []);
  return null;
}

function AppUiEditModalInner({ app }: { app: App }) {
  const [name, setName] = useState(app.name);
  const [description, setDescription] = useState(app.description);
  const { loadable: filesLoadable, setLoadable: setFilesLoadable, showLoading } = useLoadableFetch<UiSourceFile[]>(
    () => readAppSourceApi(app.id).then((res) => res.files),
    [app.id],
  );

  function updateFileContent(index: number, content: string) {
    setFilesLoadable((prev) => {
      if (prev.status !== 'loaded') return prev;
      return { status: 'loaded', data: prev.data.map((f, i) => i === index ? { ...f, content } : f) };
    });
  }

  async function handleSave(e: Event) {
    e.preventDefault();
    if (!name.trim()) return;

    if (filesLoadable.status === 'failed') {
      showToast('Cannot save: files failed to load (' + filesLoadable.error + ')', 'error');
      return;
    }
    if (filesLoadable.status !== 'loaded') {
      showToast('Cannot save: files are still loading', 'error');
      return;
    }

    const metaOk = await saveAppMetadata(app.id, name.trim(), description.trim());
    if (!metaOk) return;

    const files = filesLoadable.data;
    if (files.length > 0) {
      try {
        await writeAppSourceApi(app.id, files);
        void refreshAppUI(app.id);
      } catch (err) {
        showToast('Failed to save files: ' + errorDetail(err), 'error');
        return;
      }
    }

    closeAppForm();
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
          <FilesEditor loadable={filesLoadable} showLoading={showLoading} updateFileContent={updateFileContent} />
          <div class="form-actions">
            <button type="button" class="btn-cancel" onClick={closeAppForm}>
              Cancel
            </button>
            <button type="submit" class="btn-save" disabled={filesLoadable.status !== 'loaded'}>Save</button>
          </div>
        </div>
      </form>
    </div>
  );
}

function FilesEditor({
  loadable,
  showLoading,
  updateFileContent,
}: {
  loadable: Loadable<UiSourceFile[]>;
  showLoading: boolean;
  updateFileContent: (index: number, content: string) => void;
}) {
  if (loadable.status === 'failed') return <LoadableError noun="files" error={loadable.error} />;
  if (loadable.status !== 'loaded') return showLoading ? <div class="loading-spinner" /> : null;
  return (
    <>
      {loadable.data.map((file, i) => (
        <div class="form-group" key={file.name}>
          <label>{file.name}</label>
          <CodeTextarea value={file.content} onInput={(v) => updateFileContent(i, v)} />
        </div>
      ))}
    </>
  );
}

/** Auto-resizing textarea for code — Enter inserts newline (not submit). */
function CodeTextarea({ value, onInput }: { value: string; onInput: (v: string) => void }) {
  const ref = useRef<HTMLTextAreaElement>(null);

  useEffect(() => autoResizeTextarea(ref.current), [value]);

  return (
    <textarea
      ref={ref}
      class="auto-textarea code-textarea"
      value={value}
      onInput={(e) => onInput((e.target as HTMLTextAreaElement).value)}
      rows={3}
      spellcheck={false}
    />
  );
}
