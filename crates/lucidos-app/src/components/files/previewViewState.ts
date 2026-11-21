import { createContext } from 'preact';
import { useContext } from 'preact/hooks';
import { panelPreviewViewState, type PreviewViewState } from '../../store/store';

/** Which view state the preview components under it read. The Files panel
 *  provides nothing and gets the default, its own global set. The file preview
 *  modal provides the set it opened with. */
export const PreviewViewStateContext = createContext<PreviewViewState>(panelPreviewViewState);

export function usePreviewViewState(): PreviewViewState {
  return useContext(PreviewViewStateContext);
}

/** The path of the file the preview under it shows, as its opener wrote it:
 *  the panel overlay's path, or the modal's. A `lineScrollTarget` names the
 *  same string, so only this file's rows take it. Null outside a file preview,
 *  where no target is ever meant. */
export const PreviewPathContext = createContext<string | null>(null);

export function usePreviewPath(): string | null {
  return useContext(PreviewPathContext);
}
