
import fs from 'node:fs';
import type { ServerOptions } from 'node:https';
import tls from 'node:tls';
import { sniPicksLeaf, type PreviewTls, type TlsPairPaths } from './frontendPreviewGateway';

function readPair(paths: TlsPairPaths): { cert: Buffer; key: Buffer } {
  return { cert: fs.readFileSync(paths.cert), key: fs.readFileSync(paths.key) };
}

export function previewHttpsOptions(previewTls: PreviewTls): ServerOptions {
  const leaf = readPair(previewTls.leaf);
  if (!previewTls.user) return leaf;
  const leafContext = tls.createSecureContext(leaf);
  return {
    ...readPair(previewTls.user),
    SNICallback: (servername, done) =>
      done(null, sniPicksLeaf(servername) ? leafContext : undefined),
  };
}
