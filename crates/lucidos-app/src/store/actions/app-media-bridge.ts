import { BASE_PATH } from '../../utils/basePath';
import type { MediaPoster } from '../../../../../packages/lucidos-sdk/src/media';

const CACHE = 'lucidos-media-posters-v1';
const memory = new Map<string, MediaPoster>();
let tail: Promise<unknown> = Promise.resolve();

export function posterSize(width: number, height: number, maxSize: number): [number, number] {
  const scale = Math.min(1, Math.min(1280, maxSize) / Math.max(width, height));
  return [Math.max(1, Math.round(width * scale)), Math.max(1, Math.round(height * scale))];
}

export async function decodePoster(url: string, maxSize: number): Promise<MediaPoster> {
  const video = document.createElement('video');
  video.crossOrigin = 'anonymous';
  video.muted = true;
  video.playsInline = true;
  video.preload = 'auto';
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    await new Promise<void>((resolve, reject) => {
      timer = setTimeout(() => reject(new Error('The video poster took too long to load')), 10000);
      video.onloadeddata = () => resolve();
      video.onerror = () => reject(new Error('The video poster could not be decoded'));
      video.src = url;
      video.load();
    });
    if (!video.videoWidth || !video.videoHeight) throw new Error('The video has no image');
    const [width, height] = posterSize(video.videoWidth, video.videoHeight, maxSize);
    const canvas = document.createElement('canvas');
    canvas.width = width;
    canvas.height = height;
    const context = canvas.getContext('2d');
    if (!context) throw new Error('The video poster could not be drawn');
    try {
      context.drawImage(video, 0, 0, width, height);
      return { poster: canvas.toDataURL('image/jpeg', 0.8), width: video.videoWidth,
        height: video.videoHeight, duration: Number.isFinite(video.duration) ? video.duration : 0 };
    } finally {
      canvas.width = canvas.height = 0;
    }
  } finally {
    clearTimeout(timer);
    video.onloadeddata = video.onerror = null;
    video.removeAttribute('src');
    video.load();
  }
}

export function serialPoster<T>(work: () => Promise<T>): Promise<T> {
  const result = tail.then(work);
  tail = result.catch(() => undefined);
  return result;
}

function remember(key: string, poster: MediaPoster): void {
  memory.delete(key);
  memory.set(key, poster);
  if (memory.size > 32) memory.delete(memory.keys().next().value!);
}

async function diskCache(): Promise<Cache | null> {
  try {
    return await caches.open(CACHE);
  } catch {
    return null;
  }
}

function isPoster(value: unknown): value is MediaPoster {
  const data = value as MediaPoster | null;
  return !!data && typeof data.poster === 'string' && data.poster.startsWith('data:image/jpeg;')
    && [data.width, data.height, data.duration].every(n => typeof n === 'number' && Number.isFinite(n) && n >= 0);
}

export function mediaPoster(args: unknown, frame: HTMLIFrameElement): Promise<MediaPoster> {
  const { url: raw, maxSize } = (args ?? {}) as { url?: unknown; maxSize?: unknown };
  if (typeof raw !== 'string' || typeof maxSize !== 'number' || !Number.isFinite(maxSize) || maxSize <= 0) {
    return Promise.reject(new Error('A video poster needs a clip URL and a positive maxSize'));
  }
  const url = new URL(raw, window.location.href);
  url.hash = '';
  const local = url.origin === window.location.origin;
  const prefix = `${BASE_PATH}/`;
  const path = url.pathname.slice(prefix.length).replace(/^~cap\/[^/]+\//, '');
  if (url.username || url.password || (local
    ? !url.pathname.startsWith(prefix) || !/^(data|app)\//.test(path)
    : url.protocol !== 'https:')) {
    return Promise.reject(new Error('This clip URL cannot supply a video poster'));
  }
  const size = Math.max(1, Math.min(1280, Math.ceil(maxSize)));
  return serialPoster(async () => {
    if (!frame.isConnected) throw new Error('The video player was closed');
    const head = await fetch(url.href, { method: 'HEAD', credentials: 'omit', redirect: 'error',
      cache: 'no-cache', signal: AbortSignal.timeout(10000) });
    if (!head.ok) throw new Error('The video player cannot read this clip');
    const validator = head.headers.get('ETag') || head.headers.get('Last-Modified');
    const identity = local ? `${url.origin}${prefix}${path}${url.search}` : url.href;
    const key = validator ? `${identity}\n${validator}\n${size}` : null;
    const cached = key ? memory.get(key) : undefined;
    if (cached) return cached;
    const cache = key ? await diskCache() : null;
    const cacheUrl = key ? new URL(`${BASE_PATH}/__media-poster/${encodeURIComponent(key)}`, location.origin).href : '';
    if (cache) {
      try {
        const hit = await cache.match(cacheUrl);
        if (hit) {
          const value: unknown = await hit.json();
          if (isPoster(value)) { remember(key!, value); return value; }
        }
      } catch {  }
    }
    if (!frame.isConnected) throw new Error('The video player was closed');
    const poster = await decodePoster(url.href, size);
    if (key) remember(key, poster);
    if (cache) {
      try {
        await cache.put(cacheUrl, new Response(JSON.stringify(poster), { headers: { 'Content-Type': 'application/json' } }));
        const keys = await cache.keys();
        for (const old of keys.slice(0, Math.max(0, keys.length - 64))) await cache.delete(old);
      } catch {  }
    }
    return poster;
  });
}
