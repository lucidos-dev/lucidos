import { API, json, mutatingFetch, throwIfNotOk } from './_core';

export interface ReleaseNoticeItem {
  id: string;
  since: string;
  title: string;
  summary: string;
  body: string;
  resolved: boolean;
}

export interface ReleaseNotice {
  since: string;
  items: ReleaseNoticeItem[];
  resolved: boolean;
  action_prompt: string | null;
}

export interface ReleaseNoticeView {
  releases: ReleaseNotice[];
  owed_through: string | null;
  action_prompt: string | null;
}

export interface ReleaseNoticeAnswer {
  id: string;
  shown_ids: string[];
}

export async function releaseNotices(): Promise<ReleaseNoticeView> {
  return json(`${API}/release-notices`);
}

export async function resolveReleaseNotice(answer: ReleaseNoticeAnswer): Promise<ReleaseNoticeView> {
  const resp = await mutatingFetch(`${API}/release-notices/resolve`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(answer),
  });
  await throwIfNotOk(resp);
  return resp.json();
}
