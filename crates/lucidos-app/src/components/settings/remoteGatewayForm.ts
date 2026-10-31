/**
 * The "Add a remote gateway" form, shared by Settings → Access and the
 * Workspaces picker (ADR 0590).
 *
 * The gateway is the authority on every field, and its refusal names the fix.
 * This only holds the button until the form can be sent, and says early what
 * the gateway would refuse anyway: a plain `http` address. It also says, up
 * front, that a paired gateway is trusted like this one.
 */

import { addRemoteGateway, type RemoteGateway } from '../../api/client/control';

export interface RemoteGatewayDraft {
  address: string;
  code: string;
  name: string;
}

export const EMPTY_REMOTE_GATEWAY_DRAFT: RemoteGatewayDraft = { address: '', code: '', name: '' };

/** What the form says under a plain `http` address. */
export const HTTPS_ONLY_NOTE =
  'Gateways pair over https only, so the credential never crosses a network in clear text. '
  + 'On the other gateway, turn on remote access or set an https external address.';

/** What both forms say before anyone fills them in (ADR 0591). */
export const TRUST_NOTE =
  'Add only a gateway you run. Its pages open on this gateway\'s address, so they can do '
  + 'anything your devices can do here.';

/** Why the form cannot be sent yet, or `null` when it can. */
export type RemoteGatewayBlocker = { kind: 'incomplete' } | { kind: 'plain-http' };

/** Pure: may this draft be sent? */
export function remoteGatewayBlocker(draft: RemoteGatewayDraft): RemoteGatewayBlocker | null {
  if (/^http:\/\//i.test(draft.address.trim())) return { kind: 'plain-http' };
  if (!draft.address.trim() || !draft.code.trim() || !draft.name.trim()) {
    return { kind: 'incomplete' };
  }
  return null;
}

/** Pure: a short name as the gateway accepts it, lowercased as it is typed. */
export function shortNameInput(raw: string): string {
  return raw.toLowerCase();
}

/** Send the draft. Rejects with the gateway's own sentence. */
export function submitRemoteGateway(draft: RemoteGatewayDraft): Promise<RemoteGateway> {
  return addRemoteGateway({
    address: draft.address.trim(),
    code: draft.code.trim(),
    name: draft.name.trim(),
  });
}
