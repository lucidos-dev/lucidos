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
  return (await gatewayErrorBody(res)).reason;
}

/** The sentence, plus the `reason_kind` a typed refusal carries for the UI. */
export async function gatewayErrorBody(res: Response): Promise<{ reason: string; reasonKind: string | null }> {
  try {
    const body = await res.json();
    if (body?.error) {
      return { reason: String(body.error), reasonKind: body.reason_kind ? String(body.reason_kind) : null };
    }
  } catch {
    /* non-JSON body */
  }
  return { reason: res.statusText || `HTTP ${res.status}`, reasonKind: null };
}
