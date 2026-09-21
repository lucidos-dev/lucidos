import { API, json } from './_core';
import type { ApiResult, ModelsListResponse, ResponseStylesListResponse } from '../types';

// --- Model registry (Settings → Models) ---

export function listModels(): Promise<ModelsListResponse> {
  return json(`${API}/models`);
}

// --- Response styles (Settings -> Models -> Response style) ---

/** The merged *style library*. Read-only on purpose: an edit is a
 *  `response_styles` preference write, which is where the engine bounds-checks
 *  the document. This exists so the shipped instructions have one home, in the
 *  engine, rather than a copy here that drifts on the next reword. */
export function listResponseStyles(): Promise<ResponseStylesListResponse> {
  return json(`${API}/response-styles`);
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
