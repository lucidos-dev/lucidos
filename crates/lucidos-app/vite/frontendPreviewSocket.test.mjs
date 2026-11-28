import assert from 'node:assert/strict';
import { once } from 'node:events';
import { mkdtemp, mkdir, rm, stat, writeFile } from 'node:fs/promises';
import { request } from 'node:http';
import { connect } from 'node:net';
import { relative, resolve } from 'node:path';
import { test } from 'node:test';
import { startSocketPreview } from './frontendPreviewSocket.mjs';

test('an overlong socket path fails explicitly before Node can truncate it', async () => {
  await assert.rejects(startSocketPreview('a'.repeat(104)), /Unix socket path limit/);
});

test('Vite serves HTML, modules and HMR on one private socket with no TCP listener', async () => {
  const root = await mkdtemp(resolve('.preview-'));
  const socketPath = relative(process.cwd(), resolve(root, 'run/vite.sock'));
  const base = '/~preview/0123456789abcdef/demo/';
  let preview;
  try {
    await writeFile(resolve(root, 'index.html'), '<head></head><script type="module" src="/main.js"></script>');
    await writeFile(resolve(root, 'main.js'), 'export const value = 42;');
    preview = await startSocketPreview(socketPath, { root, base, configFile: false, logLevel: 'silent' });
    assert.equal((await stat(resolve(root, 'run'))).mode & 0o777, 0o700);
    assert.equal((await stat(socketPath)).mode & 0o777, 0o600);
    assert.equal(typeof preview.http.address(), 'string');
    assert.equal(preview.vite.httpServer, null);

    const get = path => new Promise((accept, reject) => {
      const req = request({ socketPath, path, headers: { Host: 'localhost' } }, res => {
        let body = '';
        res.on('data', chunk => { body += chunk; });
        res.on('end', () => accept({ status: res.statusCode, body }));
      });
      req.on('error', reject);
      req.end();
    });
    const html = await get(base);
    assert.equal(html.status, 200);
    assert.ok(html.body.includes(`${base}@vite/client`));
    const module = await get(`${base}main.js`);
    assert.equal(module.status, 200);
    assert.ok(module.body.includes('value = 42'));

    const socket = connect(socketPath);
    try {
      await once(socket, 'connect');
      socket.write(`GET ${base} HTTP/1.1\r\nHost: localhost\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Protocol: vite-hmr\r\n\r\n`);
      const [response] = await once(socket, 'data');
      assert.match(response.toString(), /^HTTP\/1.1 101 Switching Protocols/);
    } finally {
      socket.destroy();
    }
  } finally {
    await preview?.close();
    await rm(root, { recursive: true, force: true });
  }
});

test('a socket collision never unlinks the existing preview', async () => {
  const root = await mkdtemp(resolve('.preview-'));
  const socketPath = relative(process.cwd(), resolve(root, 'run/vite.sock'));
  await mkdir(resolve(root, 'run'));
  await writeFile(socketPath, 'owned by an existing process');
  try {
    await assert.rejects(startSocketPreview(socketPath, { root, configFile: false, logLevel: 'silent' }), { code: 'EADDRINUSE' });
    assert.ok((await stat(socketPath)).isFile());
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
