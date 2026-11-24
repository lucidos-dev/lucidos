
export interface HandshakeScriptState {
  path: string;
  exists: boolean;
  approved: boolean;
}

export function handshakeWarningFor(
  previewPath: string,
  scripts: HandshakeScriptState[],
): string | null {
  const key = previewPath.startsWith('data/') ? previewPath : `data/${previewPath}`;
  const match = scripts.find((s) => s.path === key);
  if (!match || !match.exists || match.approved) return null;
  return match.path;
}
