/**
 * The frontend preview's TLS options for Vite, from the pairs the engine
 * named (`previewTlsFromEnv`). Its own module because it needs Node's `fs`
 * and `tls`, and the app's `tsconfig` carries no Node types.
 */

import fs from 'node:fs';
import type { ServerOptions } from 'node:https';
import tls from 'node:tls';
import { sniPicksLeaf, type PreviewTls, type TlsPairPaths } from './frontendPreviewGateway';

function readPair(paths: TlsPairPaths): { cert: Buffer; key: Buffer } {
  return { cert: fs.readFileSync(paths.cert), key: fs.readFileSync(paths.key) };
}

/**
 * Serve the leaf alone, or the user cert by default and the leaf to a client
 * that names `localhost`. A client hello with no name (an IP literal) gets
 * the default, so a wide address never presents the leaf.
 */
export function previewHttpsOptions(previewTls: PreviewTls): ServerOptions {
  const leaf = readPair(previewTls.leaf);
  if (!previewTls.user) return leaf;
  const leafContext = tls.createSecureContext(leaf);
  return {
    ...readPair(previewTls.user),
    // A falsy context makes Node serve the default, the user cert.
    SNICallback: (servername, done) =>
      done(null, sniPicksLeaf(servername) ? leafContext : undefined),
  };
}
