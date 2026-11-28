/**
 * The app frames whose document has not fired `load` yet.
 *
 * WebKit shares one in-flight fetch of a subresource between every frame that
 * asks for it, and the frame that started the fetch owns it. Remove that frame
 * before the fetch finishes and it fails in every frame still waiting on it.
 * Every app frame loads `/api/v1/sdk.js` and `/api/v1/sdk-iframe.css`, so the
 * sibling draws unstyled HTML with no SDK and never reports its height.
 *
 * The host cannot see which isolated frame started which fetch, so a frame
 * that leaves while loading reloads every frame still loading.
 * Registered in `docs/temporary-measures.md` § Shared-fetch reload of app frames.
 */
const loadingFrames = new Set<() => void>();

export function trackFrameLoad(reload: () => void): void {
  loadingFrames.add(reload);
}

export function frameLoaded(reload: () => void): void {
  loadingFrames.delete(reload);
}

export function frameUnmounted(reload: () => void): void {
  if (!loadingFrames.delete(reload)) return;
  for (const other of [...loadingFrames]) other();
}
