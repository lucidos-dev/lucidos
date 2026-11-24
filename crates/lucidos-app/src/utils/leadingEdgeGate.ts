export function createLeadingEdgeGate(windowMs: number) {
  if (windowMs <= 0) {
    throw new Error(`createLeadingEdgeGate: windowMs must be > 0, got ${windowMs}`);
  }
  let lastAdmitted: number | null = null;
  return {
    allow(): boolean {
      const now = Date.now();
      if (lastAdmitted !== null && now - lastAdmitted < windowMs) return false;
      lastAdmitted = now;
      return true;
    },
  };
}
