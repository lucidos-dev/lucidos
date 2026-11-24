
export function dragCarriesFiles(e: {
  dataTransfer?: { types?: readonly string[] } | null;
}): boolean {
  return !!e.dataTransfer?.types?.includes('Files');
}

let installed = false;

export function installStrayFileDropGuard(): void {
  if (installed) return;
  installed = true;
  document.addEventListener('dragover', swallowFileDrag, { capture: true });
  document.addEventListener('drop', swallowFileDrag, { capture: true });
}

function swallowFileDrag(e: DragEvent): void {
  if (dragCarriesFiles(e)) e.preventDefault();
}
