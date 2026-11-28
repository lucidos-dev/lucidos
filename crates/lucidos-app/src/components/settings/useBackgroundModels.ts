import { useRef, useState } from 'preact/hooks';
import { getBackgroundModels } from '../../api/client';
import type { BackgroundModel } from '../../api/types';
import { useLoadableFetch } from '../../hooks/useLoadableFetch';
import { backgroundModelsVersion, chatModels, configuredProviders, preferences } from '../../store/store';

export interface BackgroundRows {
  row: (modelKey: string) => BackgroundModel | null;
  error: string | null;
  refresh: () => Promise<void>;
}

function resolutionInputs(data: Record<string, string>): string {
  return Object.keys(data)
    .filter((key) => key.startsWith('model_') || key.startsWith('reasoning_') || key === 'chat_model')
    .sort()
    .map((key) => `${key}=${data[key]}`)
    .join('\n');
}

export function useBackgroundModels(): BackgroundRows {
  const prefs = preferences.value;
  const inputs = prefs.status === 'loaded' ? resolutionInputs(prefs.data) : null;
  const providers = configuredProviders.value?.join(',') ?? null;
  const registry = chatModels.value.status === 'loaded' ? chatModels.value.data : null;
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
