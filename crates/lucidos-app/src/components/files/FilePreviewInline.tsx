import { useState, useEffect, useLayoutEffect, useMemo, useRef } from 'preact/hooks';
import { filePreviewRevision, filePreviewWrap, handshakeScriptsVersion, showToast, removeToast } from '../../store/store';
import { usePreviewViewState } from './previewViewState';
import { lucidos } from '@lucidos/sdk';
import { MarkdownDocument } from './MarkdownDocument';
import { syntaxHighlighter, highlightFileLines } from '../../utils/syntaxHighlight';
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
} from './previewFrameBridge';
import { PREVIEW_FRAME_ROLE } from '../../utils/previewFrameProtocol';
import { announceFrameGone } from '../../store/actions/frame-swipe-bridge';
import { artifactPreviewCapability, peekArtifactPreviewCapability, withPreviewCapability } from '../../store/actions/frame-capability';
import { forwardableBindings } from '../../store/actions/keybindings';
import type { ShortcutId } from '../../utils/shortcuts';
import { currentUiScale } from '../../store/actions/preferences';
import { IframeTabExit } from '../shared/IframeTabExit';

type TextPreviewBody = Exclude<DataPreviewBody, 'editor' | 'image' | 'pdf' | 'video' | 'audio' | 'unsupported'>;

export function basename(path: string): string {
  return path.split('/').pop() || '';
}

export function previewUrl(
  base: string,
  path: string,
  stamp: { path: string; rev: number } | null,
): string {
  return withPreviewRevision(base, stamp && stamp.path === path ? stamp.rev : 0);
}

function fetchText(url: string): Promise<string> {
  return fetch(url).then(r => {
    if (!r.ok) throw new ApiError(r.status, r.statusText || 'fetch failed');
    return r.text();
  });
}

function wantsPreviewCapability(body: TextPreviewBody, path: string): boolean {
  return body === 'html' && path.startsWith('artifacts/');
}

function fetchTextBody(url: string, path: string, withCapability: boolean): Promise<string> {
  const pass: Promise<unknown> = withCapability
    ? artifactPreviewCapability().catch((e) => {
      showToast(`Images and styles in ${path} may not load: ${errorDetail(e)}`, 'error');
    })
    : Promise.resolve();
  return Promise.all([fetchText(url), pass]).then(([text]) => text);
}

const checkboxToggleQueues = new Map<string, Promise<unknown>>();

export function toggleDataFileCheckbox(
  path: string, url: string, taskIndex: number, expectedCount: number,
): Promise<{ content: string; isLatest: boolean } | null> {
  const queuedAfter = checkboxToggleQueues.get(path) ?? Promise.resolve();
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
  layout: 'desktop' | 'mobile';
  modal?: boolean;
}

export function FilePreviewInline({ path, layout, modal = false }: Props) {
  const ext = previewExt(path);
  const stamp = filePreviewRevision.value;
  const url = previewUrl(lucidos.data.url(path), path, stamp);
  const revision = stamp && stamp.path === path ? stamp.rev : 0;
  const view = usePreviewViewState();
  const body = dataPreviewBody(path, {
    sourceToggle: view.source.value,
    editing: view.editing.value,
  });
  const isActiveLayout = layout === (viewportIsMobile.value ? 'mobile' : 'desktop');
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
        {isTextBody(body) && <TextContent key={path} body={body} url={url} path={path} revision={revision} servesPanel={!modal} />}
        {body === 'unsupported' && (
          <div class="empty-state">
            <p>Preview not available for <strong>.{ext}</strong> files</p>
            <a href={url} download={basename(path)}>Download file</a>
          </div>
        )}
      </div>
    </div>
  );
}

function isTextBody(body: DataPreviewBody): body is TextPreviewBody {
  return body === 'html' || body === 'markdown' || body === 'csv'
    || body === 'slides' || body === 'source';
}

function HandshakeApprovalNotice({ path }: { path: string }) {
  const [scripts, setScripts] = useState<HandshakeScriptState[]>([]);
  const load = () => {
    fetchHandshakeScripts()
      .then(setScripts)
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

export function editorToolbarState(dirty: boolean, showSaving: boolean) {
  return {
    cancelOpen: dirty,
    label: showSaving ? 'saving' : dirty ? 'save' : 'close',
    action: dirty ? 'save' : 'close',
  } as const;
}

const PRIMARY_LABELS = { save: 'Save', saving: 'Saving…', close: 'Close' } as const;

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

function FileEditor({ path, url }: { path: string; url: string }) {
  const [fetchUrl] = useState(url);
  const { loadable, showLoading } = useLoadableFetch<string>(() => fetchText(fetchUrl), [fetchUrl]);
  const [draft, setDraft] = useState<string | null>(null);
  const [baseline, setBaseline] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const delayElapsed = useDelayedFlag(saving);
  const showSaving = saving && delayElapsed;

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
      void refreshFilePreview();
    } catch (e) {
      showToast(`Failed to save: ${errorDetail(e)}`, 'error');
    } finally {
      setSaving(false);
    }
  };

  const { editing } = usePreviewViewState();
  const leaveEditMode = () => { editing.value = false; };

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

export function sourceLinesFor(content: string, ext: string, sourceMode: boolean): string[] {
  if (sourceMode) {
    const lang = ext === 'md' ? 'markdown' : ext === 'csv' ? 'text' : ext === 'svg' ? 'xml' : 'html';
    return highlightFileLines(content, lang);
  }
  if (RENDERABLE_EXTS.includes(ext)) return [];
  return highlightFileLines(content, ext);
}

function TextContent({ body, url, path, revision, servesPanel }: {
  body: TextPreviewBody; url: string; path: string; revision: number; servesPanel: boolean;
}) {
  const ext = previewExt(path);
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
    [loaded, ext, sourceMode, syntaxHighlighter.value],
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
    if (body === 'markdown') {
      const editable = isEditableDataFile(path);
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
    return <LineNumberedCode rows={sourceRows} wideLines={wideLines} />;
  }
}

const FIND_SHORTCUT: readonly ShortcutId[] = ['findInView'];

function HtmlPreviewFrame({ content, url, path, withCapability, modal }: {
  content: string; url: string; path: string; withCapability: boolean; modal: boolean;
}) {
  const scale = currentUiScale();
  const bindings = forwardableBindings(modal ? FIND_SHORTCUT : []);
  const bindingsKey = JSON.stringify(bindings);
  const declaresOwnBase = documentDeclaresBase(content);
  const baseHref = previewBaseHref(url);
  const paneSwipe = viewportIsMobile.value && !modal;
  const { srcDoc, nonce } = useMemo(() => {
    const bridge = previewBridgeConfig(bindings, paneSwipe);
    const base = withPreviewCapability(baseHref, withCapability ? peekArtifactPreviewCapability() : null);
    return {
      nonce: bridge.nonce,
      srcDoc: withPreviewBridge(withPreviewBase(withPreviewSizing(content, scale), base), bridge),
    };
  }, [content, baseHref, withCapability, scale, bindingsKey, paneSwipe]);

  const frameRef = useRef<HTMLIFrameElement>(null);
  useLayoutEffect(() => {
    const documentWindow = frameRef.current?.contentWindow ?? null;
    const onMessage = (e: MessageEvent) => {
      const frameWindow = frameRef.current?.contentWindow ?? null;
      const msg = readPreviewFrameMessage(e, frameWindow, nonce);
      if (msg) routePreviewFrameMessage(msg, { artifactPath: path, declaresOwnBase, frameWindow });
    };
    window.addEventListener('message', onMessage);
    return () => {
      window.removeEventListener('message', onMessage);
      announceFrameGone(documentWindow);
    };
  }, [nonce, path, declaresOwnBase]);

  return (
    <>
      <iframe
        ref={frameRef}
        data-role={PREVIEW_FRAME_ROLE}
        sandbox={ARTIFACT_PREVIEW_SANDBOX}
        allow={ARTIFACT_PREVIEW_ALLOW}
        srcDoc={srcDoc}
        style="width:100%;height:100%;border:none;background:#fff;"
      />
      <IframeTabExit />
    </>
  );
}

function toKnowhowId(path: string): string | null {
  const stripExt = path.endsWith('.md') ? path.slice(0, -3) : path;
  if (stripExt.startsWith('knowhow/')) return stripExt.slice('knowhow/'.length);
  if (stripExt.startsWith('system-knowhow/')) return stripExt;
  return null;
}

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
