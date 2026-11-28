export function safeRevokeObjectUrl(url: string): void {
  try { URL.revokeObjectURL(url); } catch {  }
}
