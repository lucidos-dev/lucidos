import { useState, useEffect, useLayoutEffect, useMemo, useRef } from 'preact/hooks';
import { filePreviewRevision, filePreviewSource, filePreviewWrap, filePreviewEditing, handshakeScriptsVersion, showToast, removeToast } from '../../store/store';
import { lucidos } from '@lucidos/sdk';
import { MarkdownDocument } from './MarkdownDocument';
import { highlightFileLines } from '../../utils/syntaxHighlight';
import { renderCsvTable } from '../../utils/csv';
import { PreviewImage } from './PreviewImage';
import { SlidesPreview } from './SlidesPreview';
import { viewportIsMobile } from '../../utils/viewport';
import { useLoadableFetch } from '../../hooks/useLoadableFetch';
import { useDelayedFlag } from '../../hooks/useDelayedLoading';
import { ApiError, fetchHandshakeScripts, fetchKnowhowEntries, knowhowPreviewPath, saveDataFile, type KnowhowEntry } from '../../api/client';
import { handshakeWarningFor, type HandshakeScriptState } from './handshakeApproval';
import { useVersionedRefresh } from '../../hooks/useVersionedRefresh';
import { openFilePreview, refreshFilePreview, registerPreviewTextBody, reportPreviewTextSettled } from '../../store/actions/artifacts';
import { usePanelRefresh } from '../../hooks/usePanelRefresh';
import { RENDERABLE_EXTS, isEditableDataFile } from './previewExts';
import { dataPreviewBody, previewExt, type DataPreviewBody } from './previewBody';
import { errorDetail } from '../../utils/errorDetail';
import { toggleTaskListCheckbox, countTaskListItems } from '../../utils/taskListToggle';
import { LoadableError } from '../shared/LoadableError';
import { LineNumberedCode, fileRows } from './LineNumberedCode';
import { FileSourceSkeleton, ProseSkeleton } from './previewSkeletons';
import { LoadingFade } from '../shared/LoadingFade';
import { bridgePreviewIframeShortcuts } from './previewIframeShortcuts';
import { withPreviewRevision } from './previewRevision';
import {
  documentDeclaresBase,
  handlePreviewLinkClick,
  previewBaseHref,
  withPreviewBase,
  withPreviewSizing,
} from './previewIframeLinks';
import {
  ARTIFACT_PREVIEW_ALLOW,
  ARTIFACT_PREVIEW_SANDBOX,
  previewBridgeConfig,
  readPreviewFrameMessage,
  routePreviewFrameMessage,
  withPreviewBridge,
  withPreviewCapability,
} from './previewFrameBridge';
import { PREVIEW_FRAME_ROLE } from '../../utils/previewFrameProtocol';
import { artifactPreviewCapability, peekArtifactPreviewCapability } from '../../store/actions/frame-capability';
import { forwardableBindings } from '../../store/actions/keybindings';
import type { ShortcutId } from '../../utils/shortcuts';
import { currentUiScale } from '../../store/actions/preferences';
import { IframeTabExit } from '../shared/IframeTabExit';

/** The bodies `TextContent` renders: everything reached by fetching the file as
 *  a string, rather than by pointing an element at its URL. */
type TextPreviewBody = Exclude<DataPreviewBody, 'editor' | 'image' | 'pdf' | 'video' | 'audio' | 'unsupported'>;

/** Last `/`-separated segment of `path`, or `''` for empty / trailing-slash input. */
export function basename(path: string): string {
  return path.split('/').pop() || '';
}

/** The URL the preview fetches: the file's own `/data/` URL, cache-busted only
 *  when the revision stamp names THIS file.
 *
 *  Matched on the path rather than taken from `openFilePreviewRevision`, which
 *  is what the repo preview reads: this component also renders inside the file
 *  preview modal, which can show a different file than the content pane. */
export function previewUrl(
  base: string,
  path: string,
  stamp: { path: string; rev: number } | null,
): string {
  return withPreviewRevision(base, stamp && stamp.path === path ? stamp.rev : 0);
}

/** GET a data file's body as text. A non-2xx answer rejects as an `ApiError`,
 *  so the caller's Loadable keeps the HTTP code (a 404 offers knowhow
 *  suggestions). */
function fetchText(url: string): Promise<string> {
  return fetch(url).then(r => {
    if (!r.ok) throw new ApiError(r.status, r.statusText || 'fetch failed');
    return r.text();
  });
}

/** Only an artifact gets a pass: the preview pass reaches `artifacts/` alone
 *  (ADR 0322). An HTML file elsewhere renders, with its relative refs bare. */
function wantsPreviewCapability(body: TextPreviewBody, path: string): boolean {
  return body === 'html' && path.startsWith('artifacts/');
}

/** Read the body and, when it needs one, warm the asset pass, as one load. So
 *  the frame is built once, with its pass, and never reloads to add one.
 *
 *  A pass that cannot be minted costs the images, not the document. The
 *  failure is told, and the frame renders with its relative refs bare. */
function fetchTextBody(url: string, path: string, withCapability: boolean): Promise<string> {
  const pass: Promise<unknown> = withCapability
    ? artifactPreviewCapability().catch((e) => {
      showToast(`Images and styles in ${path} may not load: ${errorDetail(e)}`, 'error');
    })
    : Promise.resolve();
  return Promise.all([fetchText(url), pass]).then(([text]) => text);
}

/** One promise chain per file path, so two checkbox toggles on the SAME file
 *  never race each other's read-modify-write. A different file always runs
 *  right away, and the map holds an entry only while a toggle for that path
 *  is in flight. */
const checkboxToggleQueues = new Map<string, Promise<unknown>>();

/** Toggle the Nth task-list checkbox in the data file at `path`, and persist
 *  it through the same write path the file editor uses.
 *
 *  Queued per file: a second toggle fired while an earlier one on the same
 *  file is still saving waits for it, rather than racing it. Two rapid
 *  toggles on the same file could otherwise race: the second reads the file
 *  before the first one's save lands. That loses the first toggle on disk
 *  and reverts its just-shown success.
 *
 *  Re-reads `url` immediately before toggling, rather than reusing the
 *  already-rendered content. The engine marks data reads `no-cache`, so this
 *  is always the current file: a concurrent edit elsewhere in it is never
 *  overwritten by a stale copy.
 *
 *  `expectedCount` is the task-item count the reader actually saw when they
 *  clicked (from the rendered content). A concurrent edit can add or remove
 *  an earlier task item, shifting every later index, without
 *  `toggleTaskListCheckbox` ever returning `null`. Comparing counts first
 *  catches that shift. It does not catch a same-count reorder, a narrower
 *  case this does not try to solve.
 *
 *  Returns the file's new full text on success, so the caller can adopt it
 *  right away instead of waiting on the save's own file-change event.
 *  Returns `null` (after toasting) on any failure, so the caller can revert
 *  its optimistic checkbox state.
 *
 *  `isLatest` tells the caller whether another toggle on this file is
 *  already queued behind this one. Only the last-queued toggle should adopt
 *  its text into shared state. An earlier one doing the same would replace
 *  the whole subtree too soon. That wipes out a later toggle's own
 *  in-flight checkbox state.
 *
 *  Toasts only on failure, under one key per file, so failures replace
 *  each other rather than stack. A successful save clears that key's
 *  toast, so an earlier failure does not outlive the save that fixed it. */
export function toggleDataFileCheckbox(
  path: string, url: string, taskIndex: number, expectedCount: number,
): Promise<{ content: string; isLatest: boolean } | null> {
  const queuedAfter = checkboxToggleQueues.get(path) ?? Promise.resolve();
  // Declared ahead of the IIFE that assigns it: the body reads `settled`
  // past its own first `await`, by which point the assignment below has
  // already run.
  let settled!: Promise<{ content: string; isLatest: boolean } | null>;
  settled = (async () => {
    await queuedAfter;
    const content = await runToggle(path, url, taskIndex, expectedCount);
    const isLatest = checkboxToggleQueues.get(path) === settled;
    if (isLatest) checkboxToggleQueues.delete(path);
    return content === null ? null : { content, isLatest };
  })();
  checkboxToggleQueues.set(path, settled);
  return settled;
}

async function runToggle(path: string, url: string, taskIndex: number, expectedCount: number): Promise<string | null> {
  const toastKey = `task-toggle:${path}`;
  try {
    const fresh = await fetchText(url);
    if (countTaskListItems(fresh) !== expectedCount) {
      showToast(`Could not update the checkbox: ${path} changed`, 'error', { key: toastKey });
      return null;
    }
    const updated = toggleTaskListCheckbox(fresh, taskIndex);
    if (updated === null) {
      showToast(`Could not update the checkbox: ${path} changed`, 'error', { key: toastKey });
      return null;
    }
    await saveDataFile(path, updated);
    removeToast(toastKey);
    return updated;
  } catch (e) {
    showToast(`Failed to update checkbox: ${errorDetail(e)}`, 'error', { key: toastKey });
    return null;
  }
}

interface Props {
  path: string;
  /** Skip mounting in the inactive dual-rendered layout — otherwise both
   *  SplitLayout and MobileSwipeContainer copies fetch and decode the file. */
  layout: 'desktop' | 'mobile';
  /** Rendered in the app-facing preview modal rather than the content pane.
   *  The modal has no header Refresh, so it takes no part in the pane's. */
  modal?: boolean;
}

export function FilePreviewInline({ path, layout, modal = false }: Props) {
  const ext = previewExt(path);
  const stamp = filePreviewRevision.value;
  const url = previewUrl(lucidos.data.url(path), path, stamp);
  const revision = stamp && stamp.path === path ? stamp.rev : 0;
  const body = dataPreviewBody(path, {
    sourceToggle: filePreviewSource.value,
    editing: filePreviewEditing.value,
  });
  const isActiveLayout = layout === (viewportIsMobile.value ? 'mobile' : 'desktop');
  // Nothing to refresh under the editor: a re-read would fight the draft.
  usePanelRefresh(`file "${path}"`, isActiveLayout && !modal && body !== 'editor' ? refreshFilePreview : null);

  if (!isActiveLayout) return null;

  return (
    <div class="file-preview-inline">
      <div class="file-preview-content">
        <HandshakeApprovalNotice path={path} />
        {body === 'editor' && <FileEditor path={path} url={url} />}
        {body === 'image' && <PreviewImage src={url} alt={path} />}
        {body === 'pdf' && <><iframe src={url} style="width:100%;height:100%;border:none;" onLoad={(e) => bridgePreviewIframeShortcuts(e.currentTarget)} /><IframeTabExit /></>}
        {body === 'video' && <video src={url} controls style="max-width:100%;max-height:100%;" />}
        {body === 'audio' && <audio src={url} controls style="width:100%;" />}
        {/* Keyed by file, so the text kept on screen during a re-read is
            always this file's and never the one opened before it. */}
        {isTextBody(body) && <TextContent key={path} body={body} url={url} path={path} revision={revision} servesPanel={!modal} />}
        {body === 'unsupported' && (
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

/** Narrows to the bodies `TextContent` owns, so the JSX above reads as one
 *  branch per body and TypeScript checks the split is exhaustive. */
function isTextBody(body: DataPreviewBody): body is TextPreviewBody {
  return body === 'html' || body === 'markdown' || body === 'csv'
    || body === 'slides' || body === 'source';
}

/** Warn when the open file is an auth handshake script the engine will not run.
 *
 *  Saving a script here cannot record authorship, because an HTTP write is
 *  indistinguishable from an app UI's (ADR 0144). So the edit silently stops
 *  the script working, and this is where the user finds out.
 *
 *  Reading approval state is safe for any caller; only approving is gated. It
 *  re-reads on `handshakeScriptsVersion`, so an approval made from the CLI,
 *  or on another device, clears the notice here (ADR 0118).
 */
function HandshakeApprovalNotice({ path }: { path: string }) {
  const [scripts, setScripts] = useState<HandshakeScriptState[]>([]);
  const load = () => {
    fetchHandshakeScripts()
      .then(setScripts)
      // Best-effort telemetry: this is a WARNING about someone else's state,
      // not the file the user asked for. A toast here would interrupt an
      // ordinary file open over a notice that has nothing to say most of the
      // time. The proxy's own 502 names the same fix if it ever matters.
      .catch((e) => console.warn('[files] handshake approval state unavailable', e));
  };
  useEffect(load, [path]);
  useVersionedRefresh(handshakeScriptsVersion.value, false, load);

  const unapproved = handshakeWarningFor(path, scripts);
  if (!unapproved) return null;
  return (
    <div class="file-preview-notice" role="status">
      <strong>This handshake script will not run.</strong> Its content is not
      approved, so the proxy refuses it. Ask the Lucidos Agent to make the
      change, or run <code>lucidos handshake approve {unapproved}</code>.
    </div>
  );
}

/** How the toolbar looks for a draft in a given state. Both buttons exist in
 *  every state (see EditorToolbar), so this answers three questions instead of
 *  picking a button set: is the Cancel slot open, which label does the primary
 *  button wear, and what does that button do.
 *
 *  `showSaving` is the DELAYED flag, never the raw one. It governs what the
 *  toolbar LOOKS like. What it can DO is the raw flag's job, see EditorToolbar.
 *
 *  Pure and exported, so the branch is checkable without a DOM. */
export function editorToolbarState(dirty: boolean, showSaving: boolean) {
  return {
    cancelOpen: dirty,
    label: showSaving ? 'saving' : dirty ? 'save' : 'close',
    action: dirty ? 'save' : 'close',
  } as const;
}

/** Every label the primary button can wear. Only the current one is rendered.
 *  `.file-editor-primary-label` reserves the widest of them, so the button
 *  holds its width as the label changes. */
const PRIMARY_LABELS = { save: 'Save', saving: 'Saving…', close: 'Close' } as const;

/** The editor's button set. No unsaved changes: a single neutral Close.
 *  Unsaved changes: red Cancel (discard) plus blue Save. Exported and kept
 *  hook-free so `vnodeToText` can render it directly in tests.
 *
 *  This mounts both buttons in both states, and the state decides how they
 *  look rather than whether they exist. Returning two different trees is what
 *  made a save snap. Preact found a fragment where a button had been, so it
 *  rebuilt the row. A rebuilt element gives a transition no two ends to run
 *  between. The send/cancel morph holds one JSX position for the same reason.
 *
 *  The two saving flags are separate on purpose. `saving` is raw and makes the
 *  buttons inert the instant the request goes out. Nobody can then hit Cancel
 *  on a write already on the wire and read the exit as a discard. `showSaving`
 *  is delayed and decides only what the row looks like. `.is-saving` is what
 *  lets a button be inert without yet wearing the disabled dim. */
export function EditorToolbar({ dirty, saving, showSaving, onClose, onCancel, onSave }: {
  dirty: boolean;
  saving: boolean;
  showSaving: boolean;
  onClose: () => void;
  onCancel: () => void;
  onSave: () => void;
}) {
  const { cancelOpen, label, action } = editorToolbarState(dirty, showSaving);
  return (
    <div class={`file-editor-actions${showSaving ? ' is-saving' : ''}`}>
      <div class={`file-editor-cancel-slot${cancelOpen ? ' is-open' : ''}`}>
        <button
          class="action-btn action-btn-danger"
          onClick={onCancel}
          disabled={saving || !cancelOpen}
        >
          Cancel
        </button>
      </div>
      <button class="action-btn" onClick={action === 'save' ? onSave : onClose} disabled={saving}>
        <span class="file-editor-primary-label">{PRIMARY_LABELS[label]}</span>
      </button>
    </div>
  );
}

/** Inline editor for a text data file. Fetches the current raw content, lets
 *  the user edit it in a textarea, and writes it back via PUT /api/v1/data.
 *  Mounted by FilePreviewInline only while `filePreviewEditing` is on for an
 *  editable path. Save/Cancel/Close live here (not in the header) so the
 *  draft state stays local to the editor.
 *
 *  The toolbar is right-aligned with Save rightmost, and Save stays on the
 *  neutral blue `action-btn` rather than the green `action-btn-confirm`: green
 *  reads as accepting something already on screen (Apply / Accept), the same
 *  reason the welcome CTA keeps the blue default.
 *
 *  A successful save does not leave edit mode. It clears the dirty state, so
 *  the toolbar settles back to the single Close button. The user decides when
 *  to return to the read view.
 *
 *  A save to a local file returns in a few tens of milliseconds. So the toolbar
 *  shows the in-flight state only once the save runs past SPINNER_DELAY_MS. The
 *  raw flag flashed it. The label swapped to Saving… and back inside a frozen
 *  box, which slid the text sideways and back for one frame. Same delay gate
 *  every loader in the app uses, and for the same reason. */
function FileEditor({ path, url }: { path: string; url: string }) {
  // Freeze the fetch URL at mount. While editing, the editor is the source of
  // truth. A later revision bump (an SSE Artifact* event naming this file) must
  // NOT refetch and tear the textarea out from under the user mid-edit. The
  // draft is already protected from being overwritten, but a refetch would
  // still flash a spinner and drop focus. Each edit session remounts FileEditor
  // (it's gated on `editing`), so a fresh url is captured per session.
  const [fetchUrl] = useState(url);
  const { loadable, showLoading } = useLoadableFetch<string>(() => fetchText(fetchUrl), [fetchUrl]);
  // `null` = not yet seeded from the fetch (distinct from an empty file `''`).
  const [draft, setDraft] = useState<string | null>(null);
  // The last-saved content. Starts as the fetched content and moves to the
  // draft on each successful save. `dirty` compares against this, not the
  // original fetch, so it tracks changes since the last save.
  const [baseline, setBaseline] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  // `useDelayedFlag` raises its flag on a timer and drops it in an EFFECT, which
  // preact runs after the paint. So the frame that ends a slow save would still
  // wear the Saving… label, on a button the raw flag has already made live.
  // Anding with the raw flag retires the two together, in one render.
  const delayElapsed = useDelayedFlag(saving);
  const showSaving = saving && delayElapsed;

  // Seed the draft and baseline once from the loaded content. A later refetch
  // must NOT clobber in-progress edits, so the guard only seeds while the
  // draft is still null.
  useEffect(() => {
    if (loadable.status === 'loaded' && draft === null) {
      setDraft(loadable.data);
      setBaseline(loadable.data);
    }
  }, [loadable, draft]);

  if (loadable.status === 'failed') {
    return <LoadableError noun="file" error={loadable.error} />;
  }
  const ready = loadable.status === 'loaded' && draft !== null;
  const dirty = draft !== baseline;

  const save = async () => {
    setSaving(true);
    try {
      await saveDataFile(path, draft ?? '');
      showToast('File saved', 'success');
      setBaseline(draft);
      void refreshFilePreview(); // bump revision so the read view re-fetches on Close
    } catch (e) {
      showToast(`Failed to save: ${errorDetail(e)}`, 'error');
    } finally {
      setSaving(false);
    }
  };

  const leaveEditMode = () => { filePreviewEditing.value = false; };

  return (
    <LoadingFade class="file-preview-fade" showSkeleton={showLoading} skeleton={<FileSourceSkeleton wideLines="wrap" />}>
      {ready && (
        <div class="file-editor">
          <div class="file-editor-toolbar">
            <EditorToolbar
              dirty={dirty}
              saving={saving}
              showSaving={showSaving}
              onClose={leaveEditMode}
              onCancel={leaveEditMode}
              onSave={save}
            />
          </div>
          {/* A save never disables the textarea. `save` writes the draft it
              captured and makes that content the baseline. So a keystroke landing
              mid-save leaves the draft ahead of it, and the toolbar correctly says
              there are unsaved changes. Disabling only dimmed the file's text for
              an instant. A delayed disable would be worse: it would take the field
              from someone typing in it. */}
          <textarea
            class="file-editor-textarea"
            value={draft ?? ''}
            spellcheck={false}
            onInput={(e) => setDraft((e.target as HTMLTextAreaElement).value)}
          />
        </div>
      )}
    </LoadingFade>
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

function TextContent({ body, url, path, revision, servesPanel }: {
  body: TextPreviewBody; url: string; path: string; revision: number; servesPanel: boolean;
}) {
  const ext = previewExt(path);
  // The Source toggle's language mapping applies exactly when the source view
  // is showing a type that HAS a rendered form. A `.rs` file is source either
  // way and takes its own grammar (see `sourceLinesFor`).
  const sourceMode = body === 'source' && RENDERABLE_EXTS.includes(ext);
  useEffect(() => (servesPanel ? registerPreviewTextBody() : undefined), [servesPanel]);
  const withCapability = wantsPreviewCapability(body, path);
  const { loadable, setLoadable, showLoading } = useLoadableFetch<string>(() => fetchTextBody(url, path, withCapability), [url, withCapability], {
    keepLoadedWhileRefetching: true,
    onSettled: servesPanel ? () => reportPreviewTextSettled(revision) : undefined,
  });
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
  const wideLines = filePreviewWrap.value ? 'wrap' : 'pan';
  // The body type is known before the read lands, so the placeholder takes
  // the shape of what will replace it.
  return (
    <LoadingFade
      class="file-preview-fade"
      showSkeleton={showLoading}
      skeleton={body === 'source' ? <FileSourceSkeleton wideLines={wideLines} /> : <ProseSkeleton />}
    >
      {loadable.status === 'loaded' && loadedBody(loadable.data)}
    </LoadingFade>
  );

  function loadedBody(content: string) {
    if (body === 'html') {
      return <HtmlPreviewFrame content={content} url={url} path={path} withCapability={withCapability} modal={!servesPanel} />;
    }
    // A markdown artifact renders into the HOST document, so its links resolve
    // against the engine-stamped `<base href="/<slug>/">`: a plain sibling link
    // like `notes.md` becomes `/<slug>/notes.md`, the SPA fallback serves the
    // shell, and the whole workspace reloads. Same routing as the HTML preview
    // (see `handlePreviewLinkClick`).
    if (body === 'markdown') {
      const editable = isEditableDataFile(path);
      // Captured per render, from what the reader actually sees: the count a
      // toggle's fresh read must still match, or it refuses to write (see
      // toggleDataFileCheckbox). A new closure each render is fine here: the
      // checkbox-wiring effect reads it through a ref, not as a dependency.
      //
      // Only `isLatest` adopts the saved text as this component's own
      // state. See `toggleDataFileCheckbox`'s docstring for why an earlier
      // toggle in the same burst must not.
      function handleToggleCheckbox(taskIndex: number): Promise<boolean> {
        return toggleDataFileCheckbox(path, url, taskIndex, countTaskListItems(content)).then((result) => {
          if (result === null) return false;
          if (result.isLatest) setLoadable({ status: 'loaded', data: result.content });
          return true;
        });
      }
      return (
        <MarkdownDocument
          content={content}
          location={{ kind: 'workspace', path }}
          onClick={(e) => handlePreviewLinkClick(e, path)}
          editable={editable}
          onToggleCheckbox={editable ? handleToggleCheckbox : undefined}
        />
      );
    }
    if (body === 'csv') return <div dangerouslySetInnerHTML={{ __html: renderCsvTable(content) }} />;
    if (body === 'slides') return <SlidesPreview content={content} />;
    // Line-numbered source: code, JSON, plain text, any unknown-but-textual file,
    // and a rich type the Source toggle asked to see raw. The same view the repo
    // preview shows, and what a navigate carrying a line needs on screen.
    return <LineNumberedCode rows={sourceRows} wideLines={wideLines} />;
  }
}

const FIND_SHORTCUT: readonly ShortcutId[] = ['findInView'];

/** An HTML artifact, rendered in a sandboxed frame at an opaque origin (ADR
 *  0322). Its scripts run, and it reaches the shell only through the message
 *  bridge in `previewFrameBridge.ts`.
 *
 *  An `about:srcdoc` document resolves relative and fragment hrefs against the
 *  HOST page's URL. `withPreviewBase` re-anchors them at the artifact's folder,
 *  carrying the asset pass behind a gateway. The bridge routes the clicks the
 *  browser would otherwise use to navigate this frame.
 *
 *  `withPreviewSizing` stamps the UI scale and a body text default, since the
 *  document inherits no root font-size either. Reading `currentUiScale()` and
 *  the shortcut bindings here subscribes to both, so a change re-stamps. */
function HtmlPreviewFrame({ content, url, path, withCapability, modal }: {
  content: string; url: string; path: string; withCapability: boolean; modal: boolean;
}) {
  const scale = currentUiScale();
  // In the modal, Mod+F stays the browser's find: the find bar searches the
  // pane behind it, not this document.
  const bindings = forwardableBindings(modal ? FIND_SHORTCUT : []);
  const bindingsKey = JSON.stringify(bindings);
  const declaresOwnBase = documentDeclaresBase(content);
  // Keyed on the base, not the URL: the URL's revision stamp changes on every
  // refresh, and an unchanged document must not reload and lose its scroll.
  const baseHref = previewBaseHref(url);
  const { srcDoc, nonce } = useMemo(() => {
    const bridge = previewBridgeConfig(bindings);
    // The latest pass, peeked: a renewal must not rebuild the document. It
    // reaches the live one over the bridge instead.
    const base = withPreviewCapability(baseHref, withCapability ? peekArtifactPreviewCapability() : null);
    return {
      nonce: bridge.nonce,
      srcDoc: withPreviewBridge(withPreviewBase(withPreviewSizing(content, scale), base), bridge),
    };
  }, [content, baseHref, withCapability, scale, bindingsKey]);

  const frameRef = useRef<HTMLIFrameElement>(null);
  // A layout effect, so the listener is in place in the commit's own task,
  // before the new document can load and post.
  useLayoutEffect(() => {
    const onMessage = (e: MessageEvent) => {
      const frameWindow = frameRef.current?.contentWindow ?? null;
      const msg = readPreviewFrameMessage(e, frameWindow, nonce);
      if (msg) routePreviewFrameMessage(msg, { artifactPath: path, declaresOwnBase, frameWindow });
    };
    window.addEventListener('message', onMessage);
    return () => window.removeEventListener('message', onMessage);
  }, [nonce, path, declaresOwnBase]);

  return (
    <>
      <iframe
        ref={frameRef}
        data-role={PREVIEW_FRAME_ROLE}
        sandbox={ARTIFACT_PREVIEW_SANDBOX}
        allow={ARTIFACT_PREVIEW_ALLOW}
        srcDoc={srcDoc}
        // `#fff` is functional rather than thematic, the token rule's second
        // carve-out. An artifact is authored against a white page and usually
        // sets no background. A themed canvas would put its black text on the
        // dark surface and leave the document unreadable.
        style="width:100%;height:100%;border:none;background:#fff;"
      />
      <IframeTabExit />
    </>
  );
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
