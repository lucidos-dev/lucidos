import { createContext } from 'preact';
import { useContext } from 'preact/hooks';
import { panelPreviewViewState, type PreviewViewState } from '../../store/store';

export const PreviewViewStateContext = createContext<PreviewViewState>(panelPreviewViewState);

export function usePreviewViewState(): PreviewViewState {
  return useContext(PreviewViewStateContext);
}

export const PreviewPathContext = createContext<string | null>(null);

export function usePreviewPath(): string | null {
  return useContext(PreviewPathContext);
}
