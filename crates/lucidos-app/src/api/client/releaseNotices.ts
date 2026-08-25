import { API, json, mutatingFetch, throwIfNotOk } from './_core';

/** One *release notice*: something this release needs the reader to know or do.
 *
 *  Authored in the repo and baked into the engine, so the text is the same on
 *  every workspace running this version. Distinct from a *release note*, which
 *  says what changed; a notice says what you have to do about it. */
export interface ReleaseNotice {
  /** Stable forever. The workspace remembers what it answered by this id. */
  id: string;
  /** The release this applies FROM, plain semver. A floor, not a stamp: the
   *  engine hides the notice until it reports this version or newer. */
  since: string;
  title: string;
  /** Raw markdown. Rendered client-side (`utils/renderMarkdown.ts`). */
  body: string;
  /** The button's label. Present exactly when `action_prompt` is. */
  action_label?: string;
  /** The sentence the button SENDS as a new message. */
  action_prompt?: string;
  /** True once this workspace has answered it. The panel keeps showing it. */
  resolved: boolean;
}

/** Everything both surfaces are drawn from, in one response so they cannot
 *  disagree about what is answered. */
export interface ReleaseNoticeView {
  /** Every notice this release has reached, oldest first. */
  notices: ReleaseNotice[];
  /** The one the modal shows, or `null` when the workspace owes nothing. */
  next_id: string | null;
}

/** See engine `GET /api/v1/release-notices`. */
export async function releaseNotices(): Promise<ReleaseNoticeView> {
  return json(`${API}/release-notices`);
}

/** Record that the user answered `id`, and get the settled list back.
 *
 *  Answering one the workspace already walked past changes nothing rather than
 *  failing: two devices showing the same modal is ordinary. See engine
 *  `POST /api/v1/release-notices/resolve`. */
export async function resolveReleaseNotice(id: string): Promise<ReleaseNoticeView> {
  const resp = await mutatingFetch(`${API}/release-notices/resolve`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ id }),
  });
  await throwIfNotOk(resp);
  return resp.json();
}
