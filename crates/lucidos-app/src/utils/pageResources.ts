/** Temporary diagnostics: docs/temporary-measures.md § Page health log. */
/** Resource proxies, never process-memory estimates. Child documents are opaque. */
import { frameResources } from './frameResources';
import { BASE_PATH } from './basePath';

export function pageResources(thread: string | null, root: Document = document): Record<string, number | null | string[]> {
  const media = Array.from(root.querySelectorAll('video, audio')) as HTMLMediaElement[];
  const videos = Array.from(root.querySelectorAll('video'));
  const images = Array.from(root.images);
  const uniqueImages = new Map<string, number>();
  for (const image of images) {
    const url = image.currentSrc || image.src;
    uniqueImages.set(url, Math.max(uniqueImages.get(url) ?? 0, image.naturalWidth * image.naturalHeight));
  }
  const pane = Array.from(root.querySelectorAll<HTMLElement>('.thread-content')).find(el =>
    Array.from(el.querySelectorAll('.chat-exchange[data-thread-id]')).some(row => row.getAttribute('data-thread-id') === thread));
  const appIds: string[] = [];
  for (const frame of root.querySelectorAll('iframe')) {
    try {
      const url = new URL(frame.src, window.location.href);
      const prefix = `${BASE_PATH}/app/`;
      if (url.origin !== window.location.origin || !url.pathname.startsWith(prefix)) continue;
      const id = url.pathname.slice(prefix.length).split('/')[0];
      if (/^[a-zA-Z0-9_-]{1,48}$/.test(id) && appIds.length < 8) appIds.push(id);
    } catch { /* Unknown frames contribute to count only. */ }
  }
  const states = Object.fromEntries([
    ...Array.from({ length: 5 }, (_, n) => [`ready${n}`, media.filter(el => el.readyState === n).length]),
    ...Array.from({ length: 4 }, (_, n) => [`network${n}`, media.filter(el => el.networkState === n).length]),
  ]);
  return {
    ...states,
    ...frameResources(root),
    appIds,
    nodes: root.getElementsByTagName('*').length,
    video: videos.length,
    audio: media.length - videos.length,
    playing: media.filter(el => !el.paused && !el.ended).length,
    loading: media.filter(el => el.readyState < 3).length,
    mediaErrors: media.filter(el => el.error !== null).length,
    videoPixels: videos.reduce((sum, el) => sum + el.videoWidth * el.videoHeight, 0),
    images: images.length,
    uniqueImagePixels: [...uniqueImages.values()].reduce((sum, pixels) => sum + pixels, 0),
    imagePixels: images.reduce((sum, el) => sum + el.naturalWidth * el.naturalHeight, 0),
    iframes: root.querySelectorAll('iframe').length,
    canvas: root.querySelectorAll('canvas').length,
    // Inspecting getContext would allocate a context. No global tracker exists.
    webglContexts: null,
    objectUrls: null,
    blobSources: [...images, ...media].filter(el => el.getAttribute('src')?.startsWith('blob:')).length,
    events: pane ? pane.querySelectorAll('[data-event-id]').length : 0,
    height: pane?.scrollHeight ?? 0,
  };
}
