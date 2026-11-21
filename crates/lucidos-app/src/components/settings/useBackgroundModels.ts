import { useRef, useState } from 'preact/hooks';
import { getBackgroundModels } from '../../api/client';
import type { BackgroundModel } from '../../api/types';
import { useLoadableFetch } from '../../hooks/useLoadableFetch';
import { backgroundModelsVersion, chatModels, configuredProviders, preferences } from '../../store/store';

/** What the engine resolves, read once for every background row on a surface.
 *  `row` is `null` until the read lands, and `error` says why it failed. */
export interface BackgroundRows {
  row: (modelKey: string) => BackgroundModel | null;
  error: string | null;
  /** Re-read the engine's answer, for the panel refresh. Settles once it
   *  lands. */
  refresh: () => Promise<void>;
}

/** The stored values a resolution reads: every model and reasoning key, and
 *  the chat model the last fallback names. A change to any of them can move a
 *  row, so it re-reads. */
function resolutionInputs(data: Record<string, string>): string {
  return Object.keys(data)
    .filter((key) => key.startsWith('model_') || key.startsWith('reasoning_') || key === 'chat_model')
    .sort()
    .map((key) => `${key}=${data[key]}`)
    .join('\n');
}

/** The model each background row runs on, from `GET /api/v1/models/background`.
 *
 *  A stored half shows at once from the preference store. An unset half waits
 *  on the engine, which resolves it against the configured providers, so no
 *  catalog default can say it. */
export function useBackgroundModels(): BackgroundRows {
  const prefs = preferences.value;
  const inputs = prefs.status === 'loaded' ? resolutionInputs(prefs.data) : null;
  const providers = configuredProviders.value?.join(',') ?? null;
  // A registry edit can move which route serves a model, so it re-reads too.
  const registry = chatModels.value.status === 'loaded' ? chatModels.value.data : null;
  // A provider refusing a model moves an unset row, so that re-reads too.
  const refused = backgroundModelsVersion.value;
  const [epoch, setEpoch] = useState(0);
  const settle = useRef<(() => void) | null>(null);
  const { loadable } = useLoadableFetch(getBackgroundModels, [inputs, providers, registry, refused, epoch], {
    keepLoadedWhileRefetching: true,
    onSettled: () => {
      settle.current?.();
      settle.current = null;
    },
  });
  const rows = loadable.status === 'loaded' ? loadable.data : null;
  return {
    row: (modelKey) => rows?.[modelKey] ?? null,
    error: loadable.status === 'failed' ? loadable.error : null,
    refresh: () => new Promise<void>((resolve) => {
      settle.current = resolve;
      setEpoch((n) => n + 1);
    }),
  };
}
