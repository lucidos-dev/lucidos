import { useState, useEffect } from 'preact/hooks';
import { artifactRevision, filePreviewSource, filePreviewEditing, openImagePopup, showToast } from '../../store/store';
import { lucidos } from '@lucidos/sdk';
import { renderMarkdown } from '../../utils/renderMarkdown';
import { syntaxHighlightJson, syntaxHighlightCode, CODE_EXTS } from '../../utils/syntaxHighlight';
import { renderCsvTable } from '../../utils/csv';
import { SlidesPreview } from './SlidesPreview';
import { isMobile, viewportIsMobile } from '../../utils/viewport';
import { useLoadableFetch } from '../../hooks/useLoadableFetch';
import { ApiError, fetchKnowhowEntries, knowhowPreviewPath, saveDataFile, type KnowhowEntry } from '../../api/client';
import { openFilePreview, refreshFilePreview } from '../../store/actions/artifacts';
import { RENDERABLE_EXTS, TEXT_EXTS, isEditableDataFile } from './previewExts';
import { errorDetail } from '../../utils/errorDetail';
import { LoadableError } from '../shared/LoadableError';

const imageExts = ['png', 'jpg', 'jpeg', 'gif', 'webp', 'svg', 'ico', 'bmp'];
// .ogg is treated as audio (Vorbis/Opus is by far the most common modern usage);
// .ogv is the video variant. Listing 'ogg' in both video and audio caused
// double-render of <video> and <audio> for the same file.
const videoExts = ['mp4', 'webm', 'ogv', 'mov'];
const audioExts = ['mp3', 'wav', 'ogg', 'flac', 'm4a'];

/** Last `/`-separated segment of `path`, or `''` for empty / trailing-slash input. */
export function basename(path: string): string {
  return path.split('/').pop() || '';
}

interface Props {
  path: string;
  /** Skip mounting in the inactive dual-rendered layout — otherwise both
   *  SplitLayout and MobileSwipeContainer copies fetch and decode the file. */
  layout: 'desktop' | 'mobile';
}

export function FilePreviewInline({ path, layout }: Props) {
  const ext = path.split('.').pop()?.toLowerCase() || '';
  const rev = artifactRevision.value;
  const base = lucidos.data.url(path);
  const url = rev ? `${base}?v=${rev}` : base;
  const sourceMode = filePreviewSource.value && RENDERABLE_EXTS.includes(ext);
  const editing = filePreviewEditing.value && isEditableDataFile(path);
  const isActiveLayout = layout === (viewportIsMobile.value ? 'mobile' : 'desktop');

  if (!isActiveLayout) return null;

  return (
    <div class="file-preview-inline">
      <div class="file-preview-content">
        {editing && <FileEditor path={path} url={url} />}
        {!editing && imageExts.includes(ext) && !(ext === 'svg' && sourceMode) && <img src={url} alt={path} style="max-width:100%;max-height:100%;object-fit:contain;" onClick={() => { if (isMobile()) openImagePopup(url); }} />}
        {!editing && ext === 'pdf' && <iframe src={url} style="width:100%;height:100%;border:none;" />}
        {!editing && videoExts.includes(ext) && <video src={url} controls style="max-width:100%;max-height:100%;" />}
        {!editing && audioExts.includes(ext) && <audio src={url} controls style="width:100%;" />}
        {!editing && (TEXT_EXTS.includes(ext) || (ext === 'svg' && sourceMode)) && <TextContent ext={ext} url={url} sourceMode={sourceMode} path={path} />}
        {!editing && !imageExts.includes(ext) && ext !== 'pdf' && !videoExts.includes(ext) && !audioExts.includes(ext) && !TEXT_EXTS.includes(ext) && (
          <div class="empty-state">
            <p>Preview not available for <strong>.{ext}</strong> files</p>
            {/* Bare `<a download>` desugars to `download={true}`, which Preact
                 serializes as `download="true"` — browser would save as `true`.
                 Empty `basename` (e.g. trailing slash) falls back to the URL /
                 Content-Disposition, which is the intent of bare `download`. */}
            <a href={url} download={basename(path)}>Download file</a>
          </div>
        )}
      </div>
    </div>
  );
}

/** Inline editor for a text data file. Fetches the current raw content, lets
 *  the user edit it in a textarea, and writes it back via PUT /api/v1/data.
 *  Mounted by FilePreviewInline only while `filePreviewEditing` is on for an
 *  editable path. Save/Cancel live here (not in the header) so the draft state
 *  stays local to the editor. */
function FileEditor({ path, url }: { path: string; url: string }) {
  // Freeze the fetch URL at mount. While editing, the editor is the source of
  // truth; a later artifactRevision bump (e.g. an SSE Artifact* event triggering
  // loadArtifacts) must NOT refetch and tear the textarea out from under the
  // user mid-edit. The draft is already protected from being overwritten, but a
  // refetch would still flash a spinner and drop focus. Each edit session
  // remounts FileEditor (it's gated on `editing`), so a fresh url is captured
  // per session.
  const [fetchUrl] = useState(url);
  const { loadable, showLoading } = useLoadableFetch<string>(
    () => fetch(fetchUrl).then(r => {
      if (!r.ok) throw new ApiError(r.status, r.statusText || 'fetch failed');
      return r.text();
    }),
    [fetchUrl],
  );
  // `null` = not yet seeded from the fetch (distinct from an empty file `''`).
  const [draft, setDraft] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  // Seed the draft once from the loaded content. A later refetch (e.g. an
  // unrelated artifactRevision bump) must NOT clobber in-progress edits, so the
  // guard only seeds while the draft is still null.
  useEffect(() => {
    if (loadable.status === 'loaded' && draft === null) setDraft(loadable.data);
  }, [loadable, draft]);

  if (loadable.status === 'failed') {
    return <LoadableError noun="file" error={loadable.error} />;
  }
  if (loadable.status !== 'loaded' || draft === null) {
    return showLoading ? <div class="loading-spinner" /> : null;
  }

  const save = async () => {
    setSaving(true);
    try {
      await saveDataFile(path, draft);
      showToast('File saved', 'success');
      filePreviewEditing.value = false;
      refreshFilePreview(); // bump revision so the rendered view re-fetches
    } catch (e) {
      showToast(`Failed to save: ${errorDetail(e)}`, 'error');
    } finally {
      setSaving(false);
    }
  };

  return (
    <div class="file-editor">
      <div class="file-editor-toolbar">
        <button class="action-btn action-btn-confirm" onClick={save} disabled={saving}>
          {saving ? 'Saving…' : 'Save'}
        </button>
        <button class="action-btn action-btn-danger" onClick={() => { filePreviewEditing.value = false; }} disabled={saving}>
          Cancel
        </button>
      </div>
      <textarea
        class="file-editor-textarea"
        value={draft}
        spellcheck={false}
        disabled={saving}
        onInput={(e) => setDraft((e.target as HTMLTextAreaElement).value)}
      />
    </div>
  );
}

function TextContent({ ext, url, sourceMode, path }: { ext: string; url: string; sourceMode: boolean; path: string }) {
  const { loadable, showLoading } = useLoadableFetch<string>(
    () => fetch(url).then(r => {
      if (!r.ok) throw new ApiError(r.status, r.statusText || 'fetch failed');
      return r.text();
    }),
    [url],
  );

  if (loadable.status === 'failed') {
    const knowhowPath = loadable.httpCode === 404 ? toKnowhowId(path) : null;
    return (
      <>
        <LoadableError noun="file" error={loadable.error} />
        {knowhowPath && <KnowhowSuggestions missingId={knowhowPath} />}
      </>
    );
  }
  if (loadable.status !== 'loaded') return showLoading ? <div class="loading-spinner" /> : null;
  const content = loadable.data;

  if (sourceMode) {
    const lang = ext === 'md' ? 'markdown' : ext === 'csv' ? 'text' : ext === 'svg' ? 'xml' : 'html';
    if (lang === 'text') return <pre class="file-preview-code">{content}</pre>;
    return <pre class="file-preview-code" dangerouslySetInnerHTML={{ __html: syntaxHighlightCode(content, lang) }} />;
  }

  if (ext === 'html' || ext === 'htm') return <iframe srcDoc={content} style="width:100%;height:100%;border:none;background:#fff;" />;
  if (ext === 'md') return <div class="response-content markdown-content" dangerouslySetInnerHTML={{ __html: renderMarkdown(content) }} />;
  if (ext === 'json') {
    try {
      const formatted = JSON.stringify(JSON.parse(content), null, 2);
      return <pre class="file-preview-code" dangerouslySetInnerHTML={{ __html: syntaxHighlightJson(formatted) }} />;
    } catch {
      return <pre class="file-preview-code">{content}</pre>;
    }
  }
  if (ext === 'csv') return <div dangerouslySetInnerHTML={{ __html: renderCsvTable(content) }} />;
  if (ext === 'slides') return <SlidesPreview content={content} />;
  if (CODE_EXTS.includes(ext)) return <pre class="file-preview-code" dangerouslySetInnerHTML={{ __html: syntaxHighlightCode(content, ext) }} />;
  return <pre class="file-preview-code">{content}</pre>;
}

/** `knowhow/lucidos-ops/foo.md` → `lucidos-ops/foo`; same for `system-knowhow/`.
 *  Returns null for paths outside those roots — only knowhow ids carry a
 *  meaningful "did you mean" lookup. */
function toKnowhowId(path: string): string | null {
  const stripExt = path.endsWith('.md') ? path.slice(0, -3) : path;
  if (stripExt.startsWith('knowhow/')) return stripExt.slice('knowhow/'.length);
  if (stripExt.startsWith('system-knowhow/')) return stripExt; // keep prefix in id
  return null;
}

/** When the user clicked a stale knowhow link, suggest entries whose id ends
 *  in the same basename. Common case: trigger references `nightly-pipeline-trigger`
 *  but the file lives at `lucidos-ops/nightly-pipeline-trigger`. */
function KnowhowSuggestions({ missingId }: { missingId: string }) {
  const { loadable } = useLoadableFetch<KnowhowEntry[]>(fetchKnowhowEntries, []);

  if (loadable.status === 'failed') {
    return <p>Could not load knowhow suggestions: {loadable.error}</p>;
  }
  if (loadable.status !== 'loaded') return null;

  const tail = basename(missingId);
  const matches = loadable.data.filter(k => k.id === tail || k.id.endsWith(`/${tail}`));
  if (matches.length === 0) return null;

  return (
    <p>
      Did you mean:{' '}
      {matches.map((m, i) => (
        <span key={m.id}>
          {i > 0 && ', '}
          <button
            type="button"
            class="accent-link"
            onClick={() => openFilePreview(knowhowPreviewPath(m.id))}
          >
            {m.id}
          </button>
        </span>
      ))}
      ?
    </p>
  );
}
