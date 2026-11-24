import { API, json } from './_core';
import type {
  ApiResult, ModelsListResponse, ResponseStylesListResponse,
} from '../types';

export interface RouteInput {
  provider: string;
  id?: string;
  context_window?: number;
}


export function listModels(): Promise<ModelsListResponse> {
  return json(`${API}/models`);
}


export function listResponseStyles(): Promise<ResponseStylesListResponse> {
  return json(`${API}/response-styles`);
}

export function createModel(body: {
  id: string;
  label: string;
  provider?: string;
  sort_order?: number;
  context_window?: number;
  routes?: RouteInput[];
  vision?: boolean;
}): Promise<ApiResult> {
  return json(`${API}/models`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  });
}

export function updateModel(
  id: string,
  body: {
    label?: string;
    provider?: string;
    sort_order?: number;
    on_chat_ladder?: boolean;
    context_window?: number | null;
    routes?: RouteInput[];
    preferred_provider?: string | null;
    vision?: boolean;
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
