import { filePreviewModal, createPreviewViewState } from '../store';
import { describeOverlayTarget, fullscreenBlocksHostOverlays } from '../appFullscreenHost';
import { resolveFileTarget } from './fileTarget';
import { handleNavigationRequest } from './navigation-request';

/** What an app asks for with `lucidos.ui.previewFile`. The field names are the
 *  `file` navigate target's own, so one object drives both calls. */
export interface FilePreviewRequest {
  file_path: string;
  /** From outside the app, so `unknown`: `resolveFileTarget` is what rejects
   *  anything that isn't a positive whole number. */
  line?: unknown;
  line_end?: unknown;
}

/** Why an app's preview request is unusable, or null when it is fine.
 *
 *  The only thing worth rejecting is a missing locator: everything else about a
 *  request degrades rather than fails (`resolveFileTarget` drops an unusable
 *  line, and a file that cannot be previewed at all renders the preview's own
 *  "not available" state, exactly as it does in the Files panel). Split out from
 *  the host's message bridge so the wire contract is checkable without a DOM. */
export function filePreviewRequestError(payload: { file_path?: unknown }): string | null {
  if (typeof payload.file_path !== 'string' || payload.file_path.length === 0) {
    return 'previewFile: file_path must be a non-empty string';
  }
  return null;
}

/** Why the host cannot put a modal on screen right now, or null when it can.
 *
 *  One case, and it is the one the host does not control: something other than
 *  the app panel holds NATIVE fullscreen. A fullscreen element is painted alone,
 *  so a modal outside its subtree cannot be seen at any z-index, and when that
 *  element is the app's own iframe (an app that called `requestFullscreen` on
 *  its own content) there is nowhere to render either, because an iframe has no
 *  DOM children. The fullscreen the HOST drives is fine: the panel is the
 *  fullscreen element and `OverlayLayer` portals the modal into it.
 *
 *  Refusing rather than opening is the whole point. A modal nobody can see, with
 *  a promise that resolved, is what made this bug silent: the app believed it
 *  worked and the reader's click did nothing. A rejection reaches the app's
 *  documented `catch { navigate('file', at) }` fallback instead.
 *
 *  Takes the verdict as a parameter (live read by default) so it is testable
 *  without a DOM. */
export function filePreviewBlockedReason(
  blocked: boolean = fullscreenBlocksHostOverlays(),
): string | null {
  return blocked
    ? `previewFile: cannot show a preview over a fullscreen element the host does not control (${describeOverlayTarget()})`
    : null;
}

let openCounter = 0;

/** Show a file over whatever the content pane is showing, without navigating.
 *
 *  The rendering is the Files panel's own (`FilePreviewInline` /
 *  `RepoFileContent`). Each open gets a fresh `PreviewViewState`, which the
 *  modal provides to those components. A Files preview behind the modal keeps
 *  its own selection, scroll target, Source toggle and edit state untouched.
 *
 *  Source view is applied iff a line was honoured, rather than inheriting the
 *  user's Source toggle: the modal renders none of the Files header's controls,
 *  so a reader who arrived with the toggle on would otherwise be stuck looking
 *  at raw markdown with no way to switch. Deterministic for the caller too:
 *  pass a line and get highlighted source, pass none and get the document. */
export function openFilePreviewModal(request: FilePreviewRequest): void {
  // One resolver, shared with the navigate router: a caller must not be able to
  // reach a file through this modal that `navigate('file', …)` would not open.
  //
  // `'file'` says this caller renders the FILE, whatever the locator names. A
  // diff is rendered from the panel's global `repoDiff` / `repoSelectedChangeId`
  // state, and loading that would rebind the Files panel behind the modal, which
  // is the navigation this whole feature exists to avoid. So a diff locator
  // previews the file itself, and `navigate` stays the way to reach the diff.
  //
  // Told as a VIEW rather than by rewriting `diff#<changeId>` into a plain file
  // locator: the rewrite made the citation's line honourable (a file view has
  // the file's own line numbers) but threw the change id away with it, leaving
  // the modal reading `HEAD` for a file whose whole point was the change. Kept,
  // the id reaches `RepoFileContent`, which fetches the end state through
  // /api/v1/changes/:id/file, the correct revision for a pending branch and an
  // applied post-merge sha alike.
  const target = resolveFileTarget(request.file_path, request.line, request.line_end, 'file');

  // A fresh view starts with `editing` off, which keeps the modal read-only:
  // the Edit toggle lives in the Files header, which the modal does not render.
  const view = createPreviewViewState();
  view.source.value = target.range !== null;
  view.selectedLines.value = target.range;
  view.lineScrollTarget.value = target.range && { path: target.path, line: target.range.start };

  filePreviewModal.value = { id: ++openCounter, path: target.path, range: target.range, view };
}

/** Dismiss the modal. Idempotent, so the Esc / backdrop / close-control paths
 *  can all call it without coordinating. Its view state goes with it. */
export function closeFilePreviewModal(): void {
  filePreviewModal.value = null;
}

/** Promote the glance into a real navigation: the same Files preview
 *  `lucidos.ui.navigate('file', …)` would have opened, at the same lines.
 *
 *  Routed through `handleNavigationRequest` rather than the HTTP navigate, so
 *  the destination and every degradation rule are the router's, not a second
 *  copy of them. Closes first, so the reader lands on the Files panel with
 *  nothing over it. */
export function escalateFilePreviewModal(): void {
  const state = filePreviewModal.peek();
  if (!state) return;
  closeFilePreviewModal();
  handleNavigationRequest({
    target: 'file',
    file_path: state.path,
    line: state.range?.start,
    line_end: state.range?.end,
  });
}
