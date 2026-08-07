/**
 * The decisions behind the `oauth_client` half of the credential form.
 *
 * Its own module (importing only types) so each is unit-testable without
 * mounting `CredentialModal`, which pulls in the credential API, the inline-form
 * store and the secret loader.
 */
import type { CredentialFields } from './credentialSecret';
import type { KnownOAuthProvider } from '../../store/types';

/** Why a submit was refused, or null when it may proceed.
 *
 *  The endpoint section has always been LABELLED "(required)" and never enforced
 *  it: the two inputs carried no `required` attribute, and the check was a pair
 *  rule ("if one, then both") that both-blank passes. So a client saved with no
 *  endpoints at all was accepted, and the failure surfaced on the NEXT press of
 *  Connect as "Missing auth_url in OAuth credentials", one screen away from the
 *  field that caused it.
 *
 *  `editing` relaxes it because a blank field on an edit means "keep what is
 *  stored" (see `buildSecret`), not "save nothing". */
export function oauthClientSubmitError(
  fields: CredentialFields,
  editing: boolean,
): string | null {
  const missing = [
    !fields.authUrl.trim() && 'Authorization URL',
    !fields.tokenUrl.trim() && 'Token URL',
  ].filter(Boolean) as string[];

  // An edit with every field blank means "keep what is stored", so it demands
  // nothing. But an edit that fills SOME fields rebuilds the whole secret from
  // the form (see `buildSecret`), so a half-filled endpoint pair really does
  // drop the other URL. That pair rule is the one thing the old check got
  // right, and dropping it here would let an edit break a working credential.
  if (editing) {
    return missing.length === 1
      ? `${missing[0]} is required: an OAuth flow cannot run without both endpoints.`
      : null;
  }

  if (!fields.clientId.trim()) return 'Client ID is required.';
  if (missing.length === 2) {
    return 'Authorization URL and Token URL are required. Pick a known provider to fill them in, or enter them by hand.';
  }
  if (missing.length === 1) {
    return `${missing[0]} is required: an OAuth flow cannot run without both endpoints.`;
  }
  return null;
}

const MISSING_FIELD_LABELS: Record<string, string> = {
  client_id: 'Client ID',
  auth_url: 'Authorization URL',
  token_url: 'Token URL',
};

/** "A", "A and B", "A, B and C". */
function joinHuman(named: string[]): string {
  return named.length === 1
    ? named[0]
    : `${named.slice(0, -1).join(', ')} and ${named[named.length - 1]}`;
}

/** Why a repair reopened the form, and what the user has to do about it NOW.
 *
 *  `formValues` is what the form currently holds for each field, which for a
 *  known provider is NOT what the stored credential holds: the registry
 *  prefilled it. That gap is the whole point. The notice used to describe the
 *  stored row and nothing else, so a repair for a known provider said "missing
 *  Authorization URL and Token URL, fill it in" above an endpoint section the
 *  registry had just filled in and collapsed. The autofill worked and the screen
 *  said it hadn't, which reads as the feature being broken rather than done.
 *
 *  So the sentence is about the form, not the row: what was wrong, and then
 *  either "save" (everything is here) or "fill in X" (it genuinely is not).
 *
 *  A value counts only once trimmed, the same rule `oauthClientSubmitError`
 *  above applies and the same one the engine's `missing_flow_fields` used to
 *  decide these fields were missing in the first place. All three have to agree,
 *  or the notice promises a save that the guard then refuses. */
export function describeMissingFields(
  missing: string[] | undefined,
  formValues: Record<string, string | undefined> = {},
): string | null {
  if (!missing?.length) return null;
  const label = (m: string) => MISSING_FIELD_LABELS[m] ?? m;
  const stillEmpty = missing.filter((m) => !formValues[m]?.trim());
  if (stillEmpty.length === 0) {
    return (
      `This registration was missing ${joinHuman(missing.map(label))}. ` +
      'Lucidos filled it in below from what it knows about this provider, ' +
      'so saving continues the connection.'
    );
  }
  return (
    `This registration is missing ${joinHuman(stillEmpty.map(label))}, so connecting ` +
    'could not start. Fill it in below and the connection continues.'
  );
}

/** Whether the client secret is expected for this provider.
 *
 *  Advisory only: the engine still derives the actual *OAuth client type* from
 *  whether a secret was saved. But a provider that only issues confidential
 *  clients makes "leave it blank" wrong advice, and the two failures (a secret
 *  sent by a public client, a secret-less redemption by a confidential one) both
 *  surface late and look nothing like their cause. */
export function secretIsExpected(row: KnownOAuthProvider | undefined): boolean {
  return row?.client_type === 'confidential';
}

/** The registry row for a typed service name, or undefined.
 *
 *  A *derived provider* name is deliberately a miss: the form asks which known
 *  provider it runs on rather than guessing from the spelling. */
export function rowForService(
  providers: KnownOAuthProvider[],
  service: string,
): KnownOAuthProvider | undefined {
  const wanted = service.trim().toLowerCase();
  if (!wanted) return undefined;
  return providers.find((p) => p.id.toLowerCase() === wanted);
}
