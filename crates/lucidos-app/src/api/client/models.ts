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
}): Promise<ApiResult> {
  return json(`${API}/models`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  });
}

export function updateModel(
  id: string,
  body: { label?: string; provider?: string; sort_order?: number; enabled?: boolean }
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
