import { useEffect, useRef } from 'preact/hooks';
import { uploadFiles } from '../../store/actions/artifacts';

export function DropZone() {
  const ref = useRef<HTMLDivElement>(null);
  const counterRef = useRef(0);

  useEffect(() => {
    function onDragEnter(e: DragEvent) {
      e.preventDefault();
      counterRef.current++;
      if (e.dataTransfer?.types.includes('Files')) {
        ref.current?.classList.add('active');
      }
    }

    function onDragLeave(e: DragEvent) {
      e.preventDefault();
      counterRef.current--;
      if (counterRef.current === 0) {
        ref.current?.classList.remove('active');
      }
    }

    function onDragOver(e: DragEvent) {
      e.preventDefault();
    }

    async function onDrop(e: DragEvent) {
      e.preventDefault();
      counterRef.current = 0;
      ref.current?.classList.remove('active');

      const files = e.dataTransfer?.files;
      if (files && files.length > 0) {
        await uploadFiles(files);
      }
    }

    document.addEventListener('dragenter', onDragEnter);
    document.addEventListener('dragleave', onDragLeave);
    document.addEventListener('dragover', onDragOver);
    document.addEventListener('drop', onDrop);

    return () => {
      document.removeEventListener('dragenter', onDragEnter);
      document.removeEventListener('dragleave', onDragLeave);
      document.removeEventListener('dragover', onDragOver);
      document.removeEventListener('drop', onDrop);
    };
  }, []);

  return (
    <div ref={ref} class="drop-zone">
      <div class="drop-zone-content">
        <span class="drop-icon">📁</span>
        <span>Drop files to import</span>
      </div>
    </div>
  );
}
