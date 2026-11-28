import { createServer as createHttpServer } from 'node:http';
import { chmod, lstat, mkdir, realpath } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createServer as createViteServer } from 'vite';

export async function startSocketPreview(socketPath, config = {}) {
  if (Buffer.byteLength(socketPath) > 103) {
    throw new Error('The frontend preview socket path exceeds the Unix socket path limit (103 bytes).');
  }
  const directory = dirname(socketPath);
  await mkdir(directory, { recursive: true, mode: 0o700 });
  const metadata = await lstat(directory);
  if (!metadata.isDirectory() || metadata.uid !== process.getuid()) {
    throw new Error('The preview socket directory must be owned by this user.');
  }
  await chmod(directory, 0o700);
  const http = createHttpServer();
  const vite = await createViteServer({
    ...config,
    server: {
      ...config.server,
      middlewareMode: true,
      hmr: { server: http },
      ws: true,
    },
  });
  http.on('request', vite.middlewares);
  try {
    await new Promise((accept, reject) => {
      http.once('error', reject);
      http.listen(socketPath, accept);
    });
    await chmod(socketPath, 0o600);
  } catch (error) {
    await vite.close();
    if (http.listening) await new Promise(accept => http.close(accept));
    throw error;
  }
  return {
    vite,
    http,
    async close() {
      await vite.close();
      await new Promise((accept, reject) => http.close(error => error ? reject(error) : accept()));
    },
  };
}

if (process.argv[1] && await realpath(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const socketPath = process.env.LUCIDOS_FRONTEND_PREVIEW_SOCKET;
  if (!socketPath || !resolve(socketPath).endsWith('/frontend-preview/vite.sock')) {
    throw new Error('The engine must supply a dedicated frontend-preview/vite.sock path.');
  }
  const preview = await startSocketPreview(socketPath);
  const close = async () => {
    await preview.close();
  };
  process.once('SIGTERM', () => { void close(); });
  process.once('SIGINT', () => { void close(); });
}
