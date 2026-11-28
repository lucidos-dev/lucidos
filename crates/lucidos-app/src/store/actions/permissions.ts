import { postCommandConsent, postMcpConsent, postMcpPermissionConsent } from '../../api/client';
import type { AllowScope } from '../thread-events';


export function resolveCodingAgentPermission(
  requestId: string,
  allowed: boolean,
  persist?: AllowScope,
  alwaysDeny?: boolean,
): Promise<void> {
  return postMcpConsent(requestId, allowed, persist, alwaysDeny);
}

export function resolveCommandPermission(
  requestId: string,
  allowed: boolean,
  persist?: AllowScope,
  alwaysDeny?: boolean,
): Promise<void> {
  return postCommandConsent(requestId, allowed, persist, alwaysDeny);
}

export function resolveMcpPermission(
  requestId: string,
  allowed: boolean,
  persist?: AllowScope,
): Promise<void> {
  return postMcpPermissionConsent(requestId, allowed, persist);
}
