/** Separate lazy chunk: ordinary replies never download the diagram library. */
import libraryUrl from 'mermaid/dist/mermaid.min.js?url';
let pending: Promise<string> | undefined;
export function loadMermaidLibrary(): Promise<string> {
  return pending ??= fetch(libraryUrl).then((response) => {
    if (!response.ok) throw new Error('Could not load the diagram renderer');
    return response.text();
  }).catch((error: unknown) => {
    pending = undefined;
    throw error;
  });
}
