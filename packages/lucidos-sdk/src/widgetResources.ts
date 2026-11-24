/** Temporary diagnostics: docs/temporary-measures.md § Page health log. */
export const WIDGET_RESOURCES_MESSAGE_TYPE = 'lucidos:ui:resources';

export interface WidgetResources {
  videoLoaded: number;
  videoPixels: number;
  images: number;
  imagePixels: number;
}

export function widgetResources(doc: Document): WidgetResources {
  const videos = Array.from(doc.querySelectorAll('video'));
  return {
    videoLoaded: videos.filter(video => video.readyState > 0).length,
    videoPixels: videos.reduce((sum, video) => sum + video.videoWidth * video.videoHeight, 0),
    images: doc.images.length,
    imagePixels: Array.from(doc.images).reduce((sum, image) => sum + image.naturalWidth * image.naturalHeight, 0),
  };
}

export function watchWidgetResources(doc: Document, report: (counts: WidgetResources) => void): () => void {
  let pending = 0;
  let last = '';
  const send = () => {
    pending = 0;
    const counts = widgetResources(doc);
    const key = JSON.stringify(counts);
    if (key === last) return;
    last = key;
    report(counts);
  };
  const schedule = () => { pending ||= setTimeout(send, 0); };
  const events = ['loadedmetadata', 'loadeddata', 'emptied', 'error', 'load', 'resize'];
  for (const event of events) doc.addEventListener(event, schedule, true);
  const observer = new MutationObserver(schedule);
  observer.observe(doc.documentElement, { subtree: true, childList: true, attributes: true, attributeFilter: ['src', 'srcset'] });
  send();
  return () => {
    clearTimeout(pending);
    observer.disconnect();
    for (const event of events) doc.removeEventListener(event, schedule, true);
  };
}
