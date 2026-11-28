export async function gatewayErrorReason(res: Response): Promise<string> {
  return (await gatewayErrorBody(res)).reason;
}

export async function gatewayErrorBody(res: Response): Promise<{ reason: string; reasonKind: string | null }> {
  try {
    const body = await res.json();
    if (body?.error) {
      return { reason: String(body.error), reasonKind: body.reason_kind ? String(body.reason_kind) : null };
    }
  } catch {
  }
  return { reason: res.statusText || `HTTP ${res.status}`, reasonKind: null };
}
