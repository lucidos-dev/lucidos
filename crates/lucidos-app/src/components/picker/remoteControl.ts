import { ApiError } from '../../api/client/_core';
import { GatewayError, listRemoteControlWorkspaces } from '../../api/client/control';
import { errorDetail } from '../../utils/errorDetail';

function refusalOf(err: unknown): { status: number; reasonKind: string | null } | null {
  if (err instanceof GatewayError) return { status: err.status, reasonKind: err.reasonKind };
  if (err instanceof ApiError) {
    const body = err.body as { reason_kind?: unknown } | undefined;
    return { status: err.httpCode, reasonKind: typeof body?.reason_kind === 'string' ? body.reason_kind : null };
  }
  return null;
}

export function hopRefusalReason(err: unknown, gateway: string): string | null {
  const refusal = refusalOf(err);
  switch (refusal?.reasonKind) {
    case 'control-not-granted':
      return `${gateway} has not allowed this gateway to control its workspaces. On ${gateway}, turn on `
        + 'the second switch on this gateway’s row under Settings → Access → Pair a gateway.';
    case 'remote-gateway-too-old':
      return `${gateway} needs an update before this gateway can control its workspaces.`;
    case 'remote-refused':
      return `${gateway} no longer accepts this gateway. Pair it again.`;
    case 'transport':
      return `${gateway} is not answering.`;
  }
  return refusal?.status === 0 ? `${gateway} is not answering.` : null;
}

export function remoteControlReason(err: unknown, gateway: string): string {
  return hopRefusalReason(err, gateway) ?? errorDetail(err);
}

export async function remoteControlBlocker(gateway: string): Promise<string | null> {
  try {
    await listRemoteControlWorkspaces(gateway);
    return null;
  } catch (err) {
    return remoteControlReason(err, gateway);
  }
}
