import { signal } from '@preact/signals';
export const remoteConnectionFailure = signal<{ reason: string; reason_kind: string } | null>(null);
