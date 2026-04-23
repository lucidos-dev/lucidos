import { useState, useEffect, useRef } from 'preact/hooks';
import { activeInlineForm, appsList, showToast } from '../../store/store';
import { closeAppForm, saveAppMetadata, refreshAppUI } from '../../store/actions/apps';
import type { App } from '../../store/types';
import { readAppSourceApi, writeAppSourceApi } from '../../api/client';
import type { UiSourceFile } from '../../api/client';
import { AutoTextarea } from '../shared/AutoTextarea';

export function AppUiEditModal() {
  const form = activeInlineForm.value;
  if (form?.type !== 'app-edit') return null;

  const { appId } = form;

  if (appsList.value.status !== 'loaded') {
    return <div class="inline-form"><div class="empty">Loading...</div></div>;
  }

  const app = appsList.value.data.find((s) => s.id === appId);
  if (!app) {
    closeAppForm();
    return null;
  }

  return <AppUiEditModalInner key={appId} app={app} />;
}

function AppUiEditModalInner({ app }: { app: App }) {
  const [name, setName] = useState(app.name);
  const [description, setDescription] = useState(app.description);
  const [files, setFiles] = useState<UiSourceFile[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState('');

  useEffect(() => {
    readAppSourceApi(app.id)
      .then((res) => {
        setFiles(res.files);
        setLoading(false);
      })
      .catch((err) => {
        setError(String(err));
        setLoading(false);
      });
  }, [app.id]);

  function updateFileContent(index: number, content: string) {
    setFiles((prev) => prev.map((f, i) => i === index ? { ...f, content } : f));
  }

  async function handleSave(e: Event) {
    e.preventDefault();
    if (!name.trim()) return;

    const metaOk = await saveAppMetadata(app.id, name.trim(), description.trim());
    if (!metaOk) return;

    if (files.length > 0) {
      try {
        await writeAppSourceApi(app.id, files);
        refreshAppUI(app.id);
      } catch (err) {
        showToast('Failed to save files: ' + String(err), 'error');
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
          {loading && <div class="empty">Loading files...</div>}
          {error && <div class="error-text">Failed to load files: {error}</div>}
          {files.map((file, i) => (
            <div class="form-group" key={file.name}>
              <label>{file.name}</label>
              <CodeTextarea value={file.content} onInput={(v) => updateFileContent(i, v)} />
            </div>
          ))}
          <div class="form-actions">
            <button type="button" class="btn-cancel" onClick={closeAppForm}>
              Cancel
            </button>
            <button type="submit" class="btn-save" disabled={loading}>Save</button>
          </div>
        </div>
      </form>
    </div>
  );
}

/** Auto-resizing textarea for code — Enter inserts newline (not submit). */
function CodeTextarea({ value, onInput }: { value: string; onInput: (v: string) => void }) {
  const ref = useRef<HTMLTextAreaElement>(null);

  function resize() {
    const el = ref.current;
    if (!el) return;
    el.style.height = 'auto';
    el.style.height = el.scrollHeight + 'px';
  }

  useEffect(resize, [value]);

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
