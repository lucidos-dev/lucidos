import { callHost } from './_bridge';
import { data } from './data';

export interface MediaPoster {
  /** A scaled JPEG data URL, suitable for an image's src. */
  poster: string;
  width: number;
  height: number;
  duration: number;
}

export const media = {
  /** Capture a still first frame without keeping this frame's video loaded.
   * maxSize is the box's long side in CSS pixels, scaled by device pixel ratio. */
  poster(clip: string, options: { maxSize?: number } = {}): Promise<MediaPoster> {
    const url = /^https:\/\//i.test(clip) ? clip : data.url(clip);
    const maxSize = (options.maxSize ?? 640) * (globalThis.devicePixelRatio || 1);
    return callHost('media.poster', { url, maxSize }, 60000) as Promise<MediaPoster>;
  },
};
