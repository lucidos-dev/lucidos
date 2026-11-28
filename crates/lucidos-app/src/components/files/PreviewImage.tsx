import { openImagePopup } from '../../store/store';

export function PreviewImage({ src, alt, onError }: { src: string; alt: string; onError?: () => void }) {
  return (
    <img
      class="preview-image"
      src={src}
      alt={alt}
      onError={onError}
      role="button"
      tabIndex={0}
      onClick={() => openImagePopup(src)}
      onKeyDown={(e) => {
        if (e.key !== 'Enter' && e.key !== ' ') return;
        e.preventDefault();
        openImagePopup(src);
      }}
    />
  );
}
