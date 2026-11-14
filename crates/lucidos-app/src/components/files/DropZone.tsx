import { useEffect, useRef } from 'preact/hooks';
import { uploadFiles } from '../../store/actions/artifacts';
import { showToast } from '../../store/store';
import { attachDroppedFilesToDraft } from '../chat/attachToDraft';
import { errorDetail } from '../../utils/errorDetail';
import { findDropZone, dispatchDrop, type DropZoneKind, type DropZoneMatch } from './dropDispatch';
import { dragCarriesFiles } from '../../utils/strayFileDrop';

const ZONE_CLASS: Record<DropZoneKind, string> = {
  attach: 'drag-over-attach',
  import: 'drag-over-import',
};
const ALL_ZONE_CLASSES = Object.values(ZONE_CLASS);

/** Document-level drag-and-drop dispatcher: routes a file drop to the zone it
 *  landed on (attach to the draft, or import into files) and highlights the zone
 *  under the cursor.
 *
 *  Drops outside any registered zone are swallowed here too, but the guarantee
 *  that a stray drop can never navigate the document belongs to
 *  `installStrayFileDropGuard` (utils/strayFileDrop.ts), which is installed for
 *  BOTH render roots rather than only wherever this component happens to be
 *  mounted. */
export function DropZone() {
  const enterCounterRef = useRef(0);
  const currentZoneRef = useRef<HTMLElement | null>(null);

  useEffect(() => {
    function applyHighlight(zone: DropZoneMatch | null) {
      const newEl = zone?.el ?? null;
      if (currentZoneRef.current === newEl) return;
      if (currentZoneRef.current) {
        currentZoneRef.current.classList.remove(...ALL_ZONE_CLASSES);
      }
      if (newEl && zone) {
        newEl.classList.add(ZONE_CLASS[zone.kind]);
      }
      currentZoneRef.current = newEl;
    }

    function clearHighlight() {
      enterCounterRef.current = 0;
      applyHighlight(null);
    }

    function onDragEnter(e: DragEvent) {
      if (!dragCarriesFiles(e)) return;
      e.preventDefault();
      enterCounterRef.current++;
      applyHighlight(findDropZone(e.target));
    }

    function onDragOver(e: DragEvent) {
      if (!dragCarriesFiles(e)) return;
      e.preventDefault();
      const zone = findDropZone(e.target);
      if (e.dataTransfer) {
        e.dataTransfer.dropEffect = zone ? 'copy' : 'none';
      }
      applyHighlight(zone);
    }

    function onDragLeave() {
      // Don't gate on dragCarriesFiles: some browsers strip dataTransfer.types
      // on dragleave, which would desync the counter and strand the highlight.
      // If we incremented in dragenter, we must decrement here unconditionally.
      if (enterCounterRef.current === 0) return;
      enterCounterRef.current--;
      if (enterCounterRef.current === 0) clearHighlight();
    }

    async function onDrop(e: DragEvent) {
      if (!dragCarriesFiles(e)) return;
      e.preventDefault();
      const target = e.target;
      const files = e.dataTransfer?.files;
      clearHighlight();
      // addEventListener ignores the returned Promise — wrap the awaited work
      // in try/catch so a rejection in dispatchDrop (or either dispatched
      // action) surfaces as a toast instead of an unhandled rejection.
      try {
        await dispatchDrop(target, files, {
          attach: attachDroppedFilesToDraft,
          import: uploadFiles,
        });
      } catch (err) {
        showToast(`Drop failed: ${errorDetail(err)}`, 'error');
      }
    }

    document.addEventListener('dragenter', onDragEnter);
    document.addEventListener('dragover', onDragOver);
    document.addEventListener('dragleave', onDragLeave);
    document.addEventListener('drop', onDrop);

    return () => {
      document.removeEventListener('dragenter', onDragEnter);
      document.removeEventListener('dragover', onDragOver);
      document.removeEventListener('dragleave', onDragLeave);
      document.removeEventListener('drop', onDrop);
    };
  }, []);

  return null;
}
