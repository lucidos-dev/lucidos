// Swallow OS file drops that land outside a drop zone, on every render root.
//
// WHAT IT PREVENTS: releasing a file over a page that never claimed the drag is
// a *navigation* in every browser engine, replacing the document with the file.
// In the packaged desktop window that is unrecoverable by hand: there is no back
// button and no address bar, so a `.enc` released a few pixels beside the
// picker's restore drop zone would take the whole app with it.
//
// WHY IT IS ITS OWN INSTALLER: `main.tsx` renders EITHER `<WorkspacePicker/>` OR
// `<App/>`. The app root has had this protection all along, as a side effect of
// `components/files/DropZone.tsx` preventing the default on every file drag it
// sees; the picker root mounts no such thing. That asymmetry was invisible while
// the desktop shell ate every drop before the page saw it (Tauri's drag-drop
// handler, now disabled in `tauri.conf.json`), and became reachable the moment
// real drops started arriving. Installing here, from `main.tsx`, makes the
// guarantee a property of the document rather than of whichever component
// happens to be mounted.
//
// CAPTURE PHASE, `preventDefault` ONLY: the guard never calls
// `stopPropagation`, so `DropZone`'s dispatcher and the picker's own drop zone
// still receive every event and still read `dataTransfer.files`. A zone that
// wants the drop gets it; only the browser's default action is cancelled.
//
// FILE DRAGS ONLY: a text or link drag carries no `Files` type and is left
// completely alone, so dragging selected text into a textarea keeps working.

/** Whether a drag event carries OS files (as opposed to text, a link, or an
 *  in-page drag). Duck-typed on `dataTransfer.types` so it can be unit-tested
 *  without a real `DragEvent`. */
export function dragCarriesFiles(e: {
  dataTransfer?: { types?: readonly string[] } | null;
}): boolean {
  return !!e.dataTransfer?.types?.includes('Files');
}

let installed = false;

/** Install the document-level stray-file-drop guard. Idempotent. */
export function installStrayFileDropGuard(): void {
  if (installed) return;
  installed = true;
  // `dragover` has to be cancelled too, not just `drop`: an uncancelled
  // `dragover` leaves the drag operation at "none", and the browser then takes
  // the drop itself instead of dispatching one we could swallow.
  document.addEventListener('dragover', swallowFileDrag, { capture: true });
  document.addEventListener('drop', swallowFileDrag, { capture: true });
}

function swallowFileDrag(e: DragEvent): void {
  if (dragCarriesFiles(e)) e.preventDefault();
}
