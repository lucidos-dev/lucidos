import { signal } from '@preact/signals';
import { h, type ComponentType } from 'preact';
import { showToast } from '../store/store';
import { reloadForStaleChunk } from '../hooks/sw-update';
import { postClientLog } from './clientLog';
import { CLIENT_BUILD_ID } from 'virtual:build-id';

export type LazyComponent<P> = ComponentType<P> & {
  preload(): Promise<boolean>;
  isLoaded(): boolean;
};

export function importedChunkOf(load: () => unknown): string | null {
  return /import\(\s*["'`]([^"'`]+)["'`]\s*\)/.exec(String(load))?.[1] ?? null;
}

export async function probeFailedChunk(file: string): Promise<void> {
  const controlled = typeof navigator !== 'undefined' && !!navigator.serviceWorker?.controller;
  const base = { importedChunk: file, build: CLIENT_BUILD_ID, controlled };
  try {
    const response = await fetch(new URL(file, new URL('assets/', document.baseURI)));
    const bytes = (await response.arrayBuffer()).byteLength;
    postClientLog('chunk', 'probe', {
      ...base,
      status: response.status,
      type: response.type,
      contentType: response.headers.get('content-type'),
      bytes,
    });
  } catch (err) {
    postClientLog('chunk', 'probe', { ...base, error: String(err).slice(0, 300) });
  }
}

function chunkLoader<T>(load: () => Promise<T>, keep: (value: T) => void, kept: () => boolean): () => Promise<boolean> {
  let loading: Promise<boolean> | null = null;
  return () => {
    if (kept()) return Promise.resolve(true);
    if (loading) return loading;
    loading = load().then(
      (value) => {
        keep(value);
        return true;
      },
      (err) => {
        loading = null;
        console.error('[lazyComponent] load failed', err);
        const importedChunk = importedChunkOf(load);
        postClientLog('chunk', 'load_failed', { message: String(err).slice(0, 300), importedChunk });
        if (!reloadForStaleChunk()) {
          showToast('Failed to load. Refresh the page to try again.', 'error');
          if (importedChunk) void probeFailedChunk(importedChunk);
        }
        return false;
      },
    );
    return loading;
  };
}

export function lazyComponent<P>(
  loader: () => Promise<ComponentType<P> | { default: ComponentType<P> }>,
  placeholder?: ComponentType<P>,
): LazyComponent<P> {
  const cached = signal<ComponentType<P> | null>(null);
  const start = chunkLoader(
    loader,
    (mod) => {
      cached.value = (typeof mod === 'object' && mod !== null && 'default' in mod
        ? mod.default
        : mod) as ComponentType<P>;
    },
    () => cached.peek() !== null,
  );

  function Lazy(props: P) {
    if (!cached.value) {
      void start();
      return placeholder ? h(placeholder, props as never) : null;
    }
    return h(cached.value, props as never);
  }
  Lazy.displayName = 'Lazy';
  Lazy.preload = start;
  Lazy.isLoaded = () => cached.peek() !== null;
  return Lazy as LazyComponent<P>;
}

export type LazyModule<M> = Pick<LazyComponent<never>, 'preload' | 'isLoaded'> & {
  get(): M;
};

export function lazyModule<M>(loader: () => Promise<M>): LazyModule<M> {
  let loaded: M | null = null;
  return {
    preload: chunkLoader(loader, (mod) => { loaded = mod; }, () => loaded !== null),
    isLoaded: () => loaded !== null,
    get() {
      if (loaded === null) throw new Error('A lazy module was read before it loaded');
      return loaded;
    },
  };
}

export type ChunkGate = Pick<LazyComponent<never>, 'preload' | 'isLoaded'>;

export function chunkGroup(first: ChunkGate, ...rest: ChunkGate[]): ChunkGate {
  return {
    preload: async () => (await first.preload())
      && (await Promise.all(rest.map((part) => part.preload()))).every(Boolean),
    isLoaded: () => first.isLoaded() && rest.every((part) => part.isLoaded()),
  };
}

export interface PendingOpen {
  readonly pending: boolean;
  cancel(): void;
}

export function whenLoaded(
  lazy: ChunkGate,
  open: () => void,
  trigger: Element | null,
  doc: Document = document,
): PendingOpen {
  if (lazy.isLoaded()) {
    open();
    return { pending: false, cancel() {} };
  }
  const onPress = (e: PointerEvent) => {
    if (!trigger?.contains(e.target as Node)) handle.cancel();
  };
  const handle = {
    pending: true,
    cancel() {
      handle.pending = false;
      doc.removeEventListener('pointerdown', onPress, true);
    },
  };
  doc.addEventListener('pointerdown', onPress, true);
  void lazy.preload().then((loaded) => {
    if (!handle.pending) return;
    handle.cancel();
    if (loaded) open();
  });
  return handle;
}
