import { useState, useEffect, useMemo } from 'preact/hooks';
import { artifactRevision, filePreviewSource, filePreviewEditing, openImagePopup, showToast } from '../../store/store';
import { lucidos } from '@lucidos/sdk';
import { renderMarkdown } from '../../utils/renderMarkdown';
import { highlightFileLines } from '../../utils/syntaxHighlight';
import { renderCsvTable } from '../../utils/csv';
import { SlidesPreview } from './SlidesPreview';
import { isMobile, viewportIsMobile } from '../../utils/viewport';
import { useLoadableFetch } from '../../hooks/useLoadableFetch';
import { ApiError, fetchKnowhowEntries, knowhowPreviewPath, saveDataFile, type KnowhowEntry } from '../../api/client';
import { openFilePreview, refreshFilePreview } from '../../store/actions/artifacts';
import { RENDERABLE_EXTS, TEXT_EXTS, IMAGE_EXTS, VIDEO_EXTS, AUDIO_EXTS, isEditableDataFile } from './previewExts';
import { errorDetail } from '../../utils/errorDetail';
import { LoadableError } from '../shared/LoadableError';
import { LineNumberedCode, fileRows } from './LineNumberedCode';
import { bridgePreviewIframeShortcuts } from './previewIframeShortcuts';
import {
  bridgePreviewIframeLinks,
  documentDeclaresBase,
  handlePreviewLinkClick,
  previewBaseHref,
  withPreviewBase,
} from './previewIframeLinks';

// SVG is text (XML) but the data-file preview shows it as an <img> by default —
// the source view is the opt-in (sourceMode), handled by the TextContent branch.
// So the <img>-eligible set here is the shared binary-image list plus svg.
function isImageLike(ext: string): boolean {
  return IMAGE_EXTS.includes(ext) || ext === 'svg';
}

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
        {!editing && isImageLike(ext) && !(ext === 'svg' && sourceMode) && <img src={url} alt={path} style="max-width:100%;max-height:100%;object-fit:contain;" onClick={() => { if (isMobile()) openImagePopup(url); }} />}
        {!editing && ext === 'pdf' && <iframe src={url} style="width:100%;height:100%;border:none;" onLoad={(e) => bridgePreviewIframeShortcuts(e.currentTarget)} />}
        {!editing && VIDEO_EXTS.includes(ext) && <video src={url} controls style="max-width:100%;max-height:100%;" />}
        {!editing && AUDIO_EXTS.includes(ext) && <audio src={url} controls style="width:100%;" />}
        {!editing && (TEXT_EXTS.includes(ext) || (ext === 'svg' && sourceMode)) && <TextContent ext={ext} url={url} sourceMode={sourceMode} path={path} />}
        {!editing && !isImageLike(ext) && ext !== 'pdf' && !VIDEO_EXTS.includes(ext) && !AUDIO_EXTS.includes(ext) && !TEXT_EXTS.includes(ext) && (
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

/** One entry of line-numbered source per line of `content`, or `[]` when this
 *  file renders richly instead (markdown, a CSV table, slides, an HTML iframe)
 *  and there is nothing to number.
 *
 *  Exported for tests. Kept pure and out of the component so the branch that
 *  decides "rendered vs source" is checkable without a DOM.
 *
 *  `highlightFileLines` closes and reopens highlight spans at each line break,
 *  which is what makes a multi-line string or comment survive being split into
 *  rows. It escapes (rather than highlights) any extension it has no language
 *  for, so plain text and markdown source come through safely escaped.
 *
 *  Nothing is reformatted on the way in, JSON included. A numbered line has to
 *  be the file's OWN line: a `path:42` citation, and the range this view hands
 *  to `currentChatContext`, both name lines in the file on disk, so numbering a
 *  pretty-printed copy would point at code that isn't there. (The repo preview
 *  has always shown JSON as written, for the same reason.) */
export function sourceLinesFor(content: string, ext: string, sourceMode: boolean): string[] {
  if (sourceMode) {
    // The Source toggle's own language mapping, unchanged: markdown and CSV
    // have no registered grammar and fall through to escaped text, SVG is XML,
    // and anything else here is HTML (which includes a `.slides` deck).
    const lang = ext === 'md' ? 'markdown' : ext === 'csv' ? 'text' : ext === 'svg' ? 'xml' : 'html';
    return highlightFileLines(content, lang);
  }
  // The rich-render set, minus nothing: an SVG only reaches here in sourceMode
  // (it renders as an <img> otherwise), so the branch above has already claimed
  // it. Derived rather than restated so a new renderable extension is covered
  // the moment it's added there.
  if (RENDERABLE_EXTS.includes(ext)) return [];
  return highlightFileLines(content, ext);
}

function TextContent({ ext, url, sourceMode, path }: { ext: string; url: string; sourceMode: boolean; path: string }) {
  const { loadable, showLoading } = useLoadableFetch<string>(
    () => fetch(url).then(r => {
      if (!r.ok) throw new ApiError(r.status, r.statusText || 'fetch failed');
      return r.text();
    }),
    [url],
  );
  const loaded = loadable.status === 'loaded' ? loadable.data : null;
  const sourceRows = useMemo(
    () => fileRows(loaded === null ? [] : sourceLinesFor(loaded, ext, sourceMode)),
    [loaded, ext, sourceMode],
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

  // The Source view wins over every rich render below: it is what the Source
  // toggle asks for, and what a navigate carrying a line sets so the cited line
  // is actually on screen to highlight.
  if (sourceMode) return <LineNumberedCode rows={sourceRows} />;

  // An `about:srcdoc` document resolves relative and fragment hrefs against the
  // HOST page's URL, so an artifact's own `#section` link or `img/chart.png` ref
  // would reach for the app shell. `withPreviewBase` re-anchors resolution at the
  // artifact's folder; `bridgePreviewIframeLinks` routes the clicks the browser
  // would otherwise use to navigate this iframe. See previewIframeLinks.ts.
  if (ext === 'html' || ext === 'htm') {
    return (
      <iframe
        srcDoc={withPreviewBase(content, previewBaseHref(url))}
        style="width:100%;height:100%;border:none;background:#fff;"
        onLoad={(e) => {
          bridgePreviewIframeShortcuts(e.currentTarget);
          bridgePreviewIframeLinks(e.currentTarget, {
            artifactPath: path,
            declaresOwnBase: documentDeclaresBase(content),
          });
        }}
      />
    );
  }
  // A markdown artifact renders into the HOST document, so its links resolve
  // against the engine-stamped `<base href="/<slug>/">`: a plain sibling link
  // like `notes.md` becomes `/<slug>/notes.md`, the SPA fallback serves the
  // shell, and the whole workspace reloads. Same routing as the HTML preview,
  // minus the fragment arm (see `PreviewLinkHost.claimFragments`).
  if (ext === 'md') {
    return (
      <div
        class="response-content markdown-content"
        onClick={(e) => handlePreviewLinkClick(e as unknown as MouseEvent, {
          doc: document,
          artifactPath: path,
          claimFragments: false,
        })}
        dangerouslySetInnerHTML={{ __html: renderMarkdown(content) }}
      />
    );
  }
  if (ext === 'csv') return <div dangerouslySetInnerHTML={{ __html: renderCsvTable(content) }} />;
  if (ext === 'slides') return <SlidesPreview content={content} />;
  // Everything else (code, JSON, plain text, and any unknown-but-textual file)
  // is line-numbered source, the same view the repo preview shows.
  return <LineNumberedCode rows={sourceRows} />;
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
