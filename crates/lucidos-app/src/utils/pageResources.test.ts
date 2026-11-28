// @vitest-environment jsdom
import { describe, it, expect } from 'vitest';
import { BASE_PATH } from './basePath';
import { pageResources } from './pageResources';
describe('resource sampling', () => {
  it('ignores links to the focused thread inside another transcript', () => {
    document.body.innerHTML = '<div class="thread-content"><div class="chat-exchange" data-thread-id="b" data-event-id="1"><a data-thread-id="a"></a></div></div><div class="thread-content"><div class="chat-exchange" data-thread-id="a" data-event-id="2"></div><div data-event-id="3"></div></div>';
    expect(pageResources('a').events).toBe(2);
    document.body.innerHTML = '';
  });
  it('counts media without inspecting URLs or allocating contexts', () => {
    document.body.innerHTML = '<video src="https://secret/?token=x"></video><audio></audio><img src="blob:secret"><canvas></canvas><iframe></iframe><div class="thread-content"><div class="chat-exchange" data-thread-id="a" data-event-id="1"></div></div><div class="thread-content"><div class="chat-exchange" data-thread-id="b" data-event-id="2"></div><div data-event-id="3"></div></div>';
    const frame = document.querySelector('iframe')!;
    frame.src = `${window.location.origin}${BASE_PATH}/app/example-player/?token=secret#private`;
    Object.defineProperties(document.querySelector('img')!, { naturalWidth: { value: 100 }, naturalHeight: { value: 50 } });
    const result = pageResources('a');
    expect(result).toMatchObject({ video: 1, audio: 1, images: 1, iframes: 1, canvas: 1, events: 1, webglContexts: null, objectUrls: null, blobSources: 1, imagePixels: 5000, appIds: ['example-player'], ready0: 2 });
    expect(JSON.stringify(result)).not.toContain('secret');
    document.body.innerHTML = '';
  });
});
