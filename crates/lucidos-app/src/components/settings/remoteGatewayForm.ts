/**
 * The "Add a remote gateway" form, shared by Settings → Access and the
 * Workspaces picker (ADR 0590).
 *
 * The gateway is the authority on every field, and its refusal names the fix.
 * This only holds the button until the form can be sent, and says early what
 * the gateway would refuse anyway: a plain `http` address. It also says, up
 * front, that a paired gateway is trusted like this one.
 */

import { headersFromDraft, type HeaderDraft } from './RemoteHeadersEditor';
import { addRemoteGateway, type RemoteGateway } from '../../api/client/control';
import { untilOwnerAllows } from '../../api/client/awaitingOwner';

export interface RemoteGatewayDraft {
  address: string;
  code: string;
  name: string;
  headers?: HeaderDraft[];
}

export const EMPTY_REMOTE_GATEWAY_DRAFT: RemoteGatewayDraft = { address: '', code: '', name: '' };

/** What the form says under a plain `http` address. */
export const HTTPS_ONLY_NOTE =
  'Gateways pair over https only, so the credential never crosses a network in clear text. '
  + 'On the other gateway, turn on remote access or set an https external address. '
  + 'Two gateways on this machine may pair over http at localhost or 127.0.0.1.';

/** What both forms say before anyone fills them in (ADR 0591). */
export const TRUST_NOTE =
  'Add only a gateway you run. Its pages open on this gateway\'s address, so they can do '
  + 'anything your devices can do here.';

/** Why the form cannot be sent yet, or `null` when it can. */
export type RemoteGatewayBlocker = { kind: 'incomplete' } | { kind: 'plain-http' };

/** Pure: is this a plain `http` address off this machine? The gateway takes
 *  `http` only for a loopback host (`remote_gateways::is_loopback_origin`). */
function isRemotePlainHttp(address: string): boolean {
  if (!/^http:\/\//i.test(address)) return false;
  let host: string;
  try {
    host = new URL(address).hostname;
  } catch {
    return true;
  }
  const loopback = host === 'localhost' || host === '[::1]' || /^127\.\d+\.\d+\.\d+$/.test(host);
  return !loopback;
}

/** Pure: may this draft be sent? */
export function remoteGatewayBlocker(draft: RemoteGatewayDraft): RemoteGatewayBlocker | null {
  if (isRemotePlainHttp(draft.address.trim())) return { kind: 'plain-http' };
  if (!draft.address.trim() || !draft.code.trim() || !draft.name.trim()) {
    return { kind: 'incomplete' };
  }
  return null;
}

/** Pure: a short name as the gateway accepts it, lowercased as it is typed. */
export function shortNameInput(raw: string): string {
  return raw.toLowerCase();
}

/** What the form says while the other gateway waits for its owner. */
export const AWAITING_OWNER_NOTE =
  'The other gateway is waiting for you. On a device paired with it, open Settings → Access '
  + 'and tap Allow. This pairs as soon as you do.';

/** Send the draft, presenting the secret again while the other gateway waits
 *  for its owner's Allow. Rejects with the gateway's own sentence. */
export function submitRemoteGateway(
  draft: RemoteGatewayDraft,
  onWaiting: () => void = () => {},
): Promise<RemoteGateway> {
  const request = {
    address: draft.address.trim(),
    code: draft.code.trim(),
    name: draft.name.trim(),
    headers: headersFromDraft(draft.headers ?? []),
  };
  return untilOwnerAllows(
    () => addRemoteGateway(request),
    onWaiting,
  );
}
