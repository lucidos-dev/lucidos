import { openImagePopup } from '../../store/store';

/** The image body of a file preview, on every surface that has one: the
 *  workspace-data preview, the repository preview, and the rendered SVG.
 *
 *  A click opens the image in the full-size popup, where the zoom controls, a
 *  wheel and a pinch take over. The pane itself only ever scales the image to
 *  fit, which leaves a tall screenshot too small to read. Enter and Space do
 *  the same, because an image carrying the pane's only action has to be
 *  reachable by keyboard. */
export function PreviewImage({ src, alt }: { src: string; alt: string }) {
  return (
    <img
      class="preview-image"
      src={src}
      alt={alt}
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
