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

/** A frame mounted and has not loaded. `reload` gives it a fresh element. */
export function trackFrameLoad(reload: () => void): void {
  loadingFrames.add(reload);
}

/** The frame's document fired `load`, so every fetch it shared has finished. */
export function frameLoaded(reload: () => void): void {
  loadingFrames.delete(reload);
}

/** The frame unmounted. If it had not loaded, it may have taken a shared
 *  fetch with it, so every frame still loading starts again. A reload is not
 *  a leave: the frames it replaces were all still loading, so all reload. */
export function frameUnmounted(reload: () => void): void {
  if (!loadingFrames.delete(reload)) return;
  for (const other of [...loadingFrames]) other();
}
