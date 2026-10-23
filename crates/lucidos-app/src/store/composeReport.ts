/** The *compose report* shape and its parser. Pure, with no store imports, so
 *  the API client can parse a report without loading the compose store. */

import type { ComposeSelectionOverride } from './composeSelections';
import type { ComposeChannelMode } from './thread-events';

/** One *compose report*: the engine's stored draft at one revision. */
export interface ComposeReport {
  /** The thread's `summary_version` after the statement that produced the
   *  report. `null` when the engine did not say, which an engine one release
   *  behind the frontend does: such a report is ordered after nothing. */
  revision: number | null;
  epoch: number | null;
  text: string;
  imageHashes: string[];
  mode: ComposeChannelMode;
  selection: ComposeSelectionOverride | null;
}

/** A compose report as the engine serializes it. Every field is optional on
 *  the read side, so a body from an older engine still parses. */
export interface ComposeReportWire {
  summary_version?: number | null;
  compose_epoch?: number | null;
  text?: string | null;
  image_hashes?: string[] | null;
  mode?: string | null;
  selection?: ComposeSelectionOverride | null;
}

export function parseComposeMode(raw: unknown): ComposeChannelMode {
  return raw === 'claude_code' || raw === 'lucidos' ? raw : null;
}

export function parseComposeReport(wire: ComposeReportWire): ComposeReport {
  return {
    revision: typeof wire.summary_version === 'number' ? wire.summary_version : null,
    epoch: typeof wire.compose_epoch === 'number' ? wire.compose_epoch : null,
    text: wire.text ?? '',
    imageHashes: Array.isArray(wire.image_hashes) ? [...wire.image_hashes] : [],
    mode: parseComposeMode(wire.mode),
    selection: wire.selection ?? null,
  };
}

/** Whether two image-hash lists name the same images in the same order. */
export function sameHashes(a: readonly string[], b: readonly string[]): boolean {
  return a.length === b.length && a.every((h, i) => h === b[i]);
}
