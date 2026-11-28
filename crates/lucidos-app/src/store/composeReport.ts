
import type { ComposeSelectionOverride } from './composeSelections';
import type { ComposeChannelMode } from './thread-events';

import type { ComposeRestoration } from '../generated/thread-event-wire';
export type { ComposeRestoration } from '../generated/thread-event-wire';

export interface ParsedComposeReport {
  revision: number | null;
  epoch: number | null;
  text: string;
  imageHashes: string[];
  mode: ComposeChannelMode;
  selection: ComposeSelectionOverride | null;
  restorations?: ComposeRestoration[];
}

export interface ComposeReportWire {
  summary_version?: number | null;
  compose_epoch?: number | null;
  text?: string | null;
  image_hashes?: string[] | null;
  mode?: string | null;
  selection?: ComposeSelectionOverride | null;
  restorations?: ComposeRestoration[];
}

export function parseComposeMode(raw: unknown): ComposeChannelMode {
  return raw === 'claude_code' || raw === 'lucidos' ? raw : null;
}

export function parseComposeReport(wire: ComposeReportWire): ParsedComposeReport {
  return {
    revision: typeof wire.summary_version === 'number' ? wire.summary_version : null,
    epoch: typeof wire.compose_epoch === 'number' ? wire.compose_epoch : null,
    text: wire.text ?? '',
    imageHashes: Array.isArray(wire.image_hashes) ? [...wire.image_hashes] : [],
    mode: parseComposeMode(wire.mode),
    selection: wire.selection ?? null,
    restorations: wire.restorations ?? [],
  };
}

export function sameHashes(a: readonly string[], b: readonly string[]): boolean {
  return a.length === b.length && a.every((h, i) => h === b[i]);
}
