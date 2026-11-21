import { GatewayError, type GatewayUser, type GatewayUsersResponse, type RelayedUser } from './control';

/** See "User rename wire aliases" in docs/temporary-measures.md. */
interface LegacyGatewayUsersResponse {
  operators: GatewayUser[];
  relayed: (Omit<RelayedUser, 'user_id'> & { operator_id: string })[];
}

/** Loaded only when the local gateway still serves the older wire contract. */
export async function legacyGatewayUsersCall(
  path: string,
  body: unknown,
  request: (path: string, body: unknown) => Promise<LegacyGatewayUsersResponse>,
): Promise<GatewayUsersResponse> {
  const assignment = path.startsWith('/devices/');
  const legacyPath = assignment ? path.replace(/\/user$/, '/operator') : path.replace(/^\/users/, '/operators');
  const legacyBody = assignment && typeof body === 'object' && body !== null && 'user_id' in body
    ? { operator_id: body.user_id }
    : body;
  const legacy = await request(legacyPath, legacyBody);
  return {
    users: legacy.operators,
    relayed: legacy.relayed.map(({ operator_id, ...user }) => ({ ...user, user_id: operator_id })),
  };
}

/** Keep user-management request adaptation out of the initial shell bundle. */
export async function requestGatewayUsers(
  gateway: string | null,
  path: string,
  method: string,
  body: unknown,
  authJson: <T>(gateway: string | null, path: string, init?: RequestInit) => Promise<T>,
): Promise<GatewayUsersResponse> {
  const request = (requestBody: unknown): RequestInit => ({
    method,
    headers: requestBody === undefined ? undefined : { 'Content-Type': 'application/json' },
    body: requestBody === undefined ? undefined : JSON.stringify(requestBody),
  });
  try {
    return await authJson<GatewayUsersResponse>(gateway, path, request(body));
  } catch (error) {
    if (gateway !== null || !(error instanceof GatewayError) || !error.isAbsent) throw error;
    return legacyGatewayUsersCall(path, body, (legacyPath, legacyBody) =>
      authJson(null, legacyPath, request(legacyBody)));
  }
}
