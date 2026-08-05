import {
  filePreviewModal,
  filePreviewSource,
  filePreviewEditing,
  selectedLines,
  lineScrollTarget,
} from '../store';
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

/** Puts the four preview view-state signals back the way the modal found them.
 *  Null while no modal is open. Set on the FIRST open only: a second
 *  `previewFile` replacing a showing modal must restore the state from before
 *  the first one, not the state the first one applied. */
let restoreViewState: (() => void) | null = null;

/** Show a file over whatever the content pane is showing, without navigating.
 *
 *  The rendering is the Files panel's own (`FilePreviewInline` /
 *  `RepoFileContent`), and those components read the preview view state from
 *  signals: `selectedLines` and `lineScrollTarget` are what `LineNumberedCode`
 *  highlights and scrolls to, and `filePreviewSource` is what picks source over
 *  a rendered document. So the modal borrows those signals for its lifetime and
 *  hands them back on close. That is safe because the panel's own preview is
 *  never mounted at the same time (the modal is reachable only from an app
 *  iframe, so the content pane is showing that app), and the hand-back is what
 *  keeps the user's persisted Source toggle and any panel-side selection exactly
 *  as they left them.
 *
 *  Source view is applied iff a line was honoured, rather than inheriting the
 *  user's Source toggle: the modal renders none of the Files header's controls,
 *  so a reader who arrived with the toggle on would otherwise be stuck looking
 *  at raw markdown with no way to switch. Deterministic for the app author too:
 *  pass a line and get highlighted source, pass none and get the document. */
export function openFilePreviewModal(request: FilePreviewRequest): void {
  // One resolver, shared with the navigate router: an app must not be able to
  // reach a file through this modal that `navigate('file', …)` would not open.
  //
  // `'file'` says this caller renders the FILE, whatever the locator names. A
  // diff is rendered from the panel's global `repoDiff` / `repoSelectedChangeId`
  // state, and loading that would rebind the Files panel behind the app, which
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

  if (!restoreViewState) {
    const source = filePreviewSource.peek();
    const selection = selectedLines.peek();
    const scroll = lineScrollTarget.peek();
    const editing = filePreviewEditing.peek();
    restoreViewState = () => {
      filePreviewSource.value = source;
      selectedLines.value = selection;
      lineScrollTarget.value = scroll;
      filePreviewEditing.value = editing;
    };
  }

  // Read-only: `FilePreviewInline` mounts its editor off this signal, and the
  // Edit toggle lives in the Files header, which the modal does not render.
  filePreviewEditing.value = false;
  filePreviewSource.value = target.range !== null;
  selectedLines.value = target.range;
  lineScrollTarget.value = target.range?.start ?? null;

  filePreviewModal.value = { id: ++openCounter, path: target.path, range: target.range };
}

/** Dismiss the modal and hand the borrowed view state back. Idempotent, so the
 *  Esc / backdrop / close-control paths can all call it without coordinating.
 *
 *  Gated on the BORROW, not on the modal signal: the borrow is what this
 *  function exists to undo, so making it the one "is a modal open" test keeps
 *  the pair from drifting. Gating on the signal instead would let a caller that
 *  cleared `filePreviewModal` directly strand the snapshot, and the next open
 *  would then skip its own (see the guard there) and restore a stale pair.
 *
 *  `navigated: true` says the content pane has moved on and the destination
 *  already OWNS the view state: `openFilePreview` clears the selection, the
 *  scroll target and the source toggle and only then sets `panelOverlay`, which
 *  is what the modal's watcher sees. Handing the snapshot back on top of that
 *  would overwrite the destination's cleared state with the panel's pre-modal
 *  one, which is precisely the cross-file highlight leak the openers clear it to
 *  prevent (a range picked in one file highlighting whatever rows sit at those
 *  numbers in the next). So that path drops the borrow instead of applying it.
 *  Every other dismissal restores, the escalation included: it closes BEFORE it
 *  navigates, so the router's writes land after the hand-back, not under it. */
export function closeFilePreviewModal(opts?: { navigated?: boolean }): void {
  if (restoreViewState === null) return;
  const restore = restoreViewState;
  restoreViewState = null;
  filePreviewModal.value = null;
  if (!opts?.navigated) restore();
}

/** Promote the glance into a real navigation: the same Files preview
 *  `lucidos.ui.navigate('file', …)` would have opened, at the same lines.
 *
 *  Routed through `handleNavigationRequest` rather than the HTTP navigate, so
 *  the destination and every degradation rule are the router's, not a second
 *  copy of them. Closes first, so the reader lands on the Files panel with
 *  nothing over it (and the borrowed view state is handed back before the
 *  router sets its own). */
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
