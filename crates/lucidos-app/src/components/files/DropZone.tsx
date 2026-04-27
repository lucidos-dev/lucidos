import { useEffect, useRef } from 'preact/hooks';
import { uploadFiles } from '../../store/actions/artifacts';
import { attachDroppedFilesToDraft } from '../chat/attachToDraft';
import { findDropZone, dispatchDrop, type DropZoneKind, type DropZoneMatch } from './dropDispatch';

const ZONE_CLASS: Record<DropZoneKind, string> = {
  attach: 'drag-over-attach',
  import: 'drag-over-import',
};
const ALL_ZONE_CLASSES = Object.values(ZONE_CLASS);

/** Document-level drag-and-drop dispatcher.
 *
 *  Drops outside any registered zone are swallowed (preventDefault) so the
 *  browser doesn't navigate to the dropped file — without that, releasing
 *  outside the thread/files panes would open the file in a new tab. */
export function DropZone() {
  const enterCounterRef = useRef(0);
  const currentZoneRef = useRef<HTMLElement | null>(null);

  useEffect(() => {
    function isFileDrag(e: DragEvent): boolean {
      return !!e.dataTransfer?.types?.includes('Files');
    }

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
      if (!isFileDrag(e)) return;
      e.preventDefault();
      enterCounterRef.current++;
      applyHighlight(findDropZone(e.target));
    }

    function onDragOver(e: DragEvent) {
      if (!isFileDrag(e)) return;
      e.preventDefault();
      const zone = findDropZone(e.target);
      if (e.dataTransfer) {
        e.dataTransfer.dropEffect = zone ? 'copy' : 'none';
      }
      applyHighlight(zone);
    }

    function onDragLeave(_e: DragEvent) {
      // Don't gate on isFileDrag — some browsers strip dataTransfer.types on
      // dragleave, which would desync the counter and strand the highlight.
      // If we incremented in dragenter, we must decrement here unconditionally.
      if (enterCounterRef.current === 0) return;
      enterCounterRef.current--;
      if (enterCounterRef.current === 0) clearHighlight();
    }

    async function onDrop(e: DragEvent) {
      if (!isFileDrag(e)) return;
      e.preventDefault();
      const target = e.target;
      const files = e.dataTransfer?.files;
      clearHighlight();
      await dispatchDrop(target, files, {
        attach: attachDroppedFilesToDraft,
        import: uploadFiles,
      });
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
