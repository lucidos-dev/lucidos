/** Stamping a preview's cache-buster onto the URL it fetches.
 *
 *  Shared by the two previews because they build their URLs differently and
 *  must still bust the cache identically. A repo URL carries `?path=…&ref=…`,
 *  and `lucidos.data.url` answers `/app/<id>/…?thread_id=…` for an app's own
 *  asset under a WIP preview. So neither caller can hardcode the separator.
 */

/** Append the preview revision to `url`, whether or not it has a query.
 *
 *  A zero revision returns `url` byte for byte. That is what the whole
 *  path-scoped invalidation rests on: this URL is the `src` of a `<video>` or
 *  `<img>`, so any change to it reloads the element and restarts playback. */
export function withPreviewRevision(url: string, rev: number): string {
  if (!rev) return url;
  return `${url}${url.includes('?') ? '&' : '?'}v=${rev}`;
}
