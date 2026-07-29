import { API, json } from './_core';
import type { ApiResult, ModelsListResponse } from '../types';

// --- Model registry (Settings → Models) ---

export function listModels(): Promise<ModelsListResponse> {
  return json(`${API}/models`);
}

export function createModel(body: {
  id: string;
  label: string;
  provider: string;
  sort_order?: number;
  context_window?: number;
}): Promise<ApiResult> {
  return json(`${API}/models`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  });
}

export function updateModel(
  id: string,
  // `context_window: null` CLEARS the declared window (back to inferring from
  // the model id); omitting the key leaves the stored value alone.
  body: {
    label?: string;
    provider?: string;
    sort_order?: number;
    enabled?: boolean;
    context_window?: number | null;
  }
): Promise<ApiResult> {
  return json(`${API}/models?id=${encodeURIComponent(id)}`, {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  });
}

export function deleteModelApi(id: string): Promise<ApiResult> {
  return json(`${API}/models?id=${encodeURIComponent(id)}`, {
    method: 'DELETE',
  });
}
