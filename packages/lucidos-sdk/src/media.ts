import { callHost } from './_bridge';
import { data } from './data';

export interface MediaPoster {
  poster: string;
  width: number;
  height: number;
  duration: number;
}

export const media = {
  poster(clip: string, options: { maxSize?: number } = {}): Promise<MediaPoster> {
    const url = /^https:\/\//i.test(clip) ? clip : data.url(clip);
    const maxSize = (options.maxSize ?? 640) * (globalThis.devicePixelRatio || 1);
    return callHost('media.poster', { url, maxSize }, 60000) as Promise<MediaPoster>;
  },
};
