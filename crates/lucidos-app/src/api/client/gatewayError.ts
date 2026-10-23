/**
 * The reason a gateway call failed, as the gateway itself stated it.
 *
 * Every `/~/…` handler answers an error through the gateway's own `ApiError`,
 * which is an `{"error": "…"}` body. Reporting `res.statusText` instead throws
 * away the sentence written for the user, and "Bad Request" is the one message
 * that never says what to do.
 *
 * A leaf on purpose. Both gateway-facing clients need it, and neither may pull
 * in the store to get it: the picker runs them before any workspace exists.
 */
export async function gatewayErrorReason(res: Response): Promise<string> {
  try {
    const body = await res.json();
    if (body?.error) return String(body.error);
  } catch {
    /* non-JSON body */
  }
  return res.statusText || `HTTP ${res.status}`;
}
