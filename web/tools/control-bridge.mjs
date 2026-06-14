// HTTP -> UDS bridge for the control plane.
//
// Listens on 127.0.0.1:BRIDGE_PORT and forwards every request verbatim to the
// daemon's control.sock, injecting the `x-postern-control-token` header (read
// from the file at POSTERN_CONTROL_TOKEN). This is (a) the web-private delivery
// form's bridge — the browser can't speak UDS, so httpTransport's /v1/* fetches
// are proxied here — and (b) the harness that lets headless Playwright drive the
// real frontend against the real backend for automated per-page verification.
//
// Env: POSTERN_CONTROL_SOCK (required), POSTERN_CONTROL_TOKEN (token file path),
// BRIDGE_PORT (default 8787).

import http from 'node:http';
import fs from 'node:fs';

const PORT = Number(process.env.BRIDGE_PORT || 8787);
const SOCK = process.env.POSTERN_CONTROL_SOCK;
const TOKEN_PATH = process.env.POSTERN_CONTROL_TOKEN;

if (!SOCK) {
  console.error('control-bridge: POSTERN_CONTROL_SOCK not set');
  process.exit(1);
}
const token = TOKEN_PATH ? fs.readFileSync(TOKEN_PATH, 'utf8').trim() : '';

const server = http.createServer((req, res) => {
  const chunks = [];
  req.on('data', (c) => chunks.push(c));
  req.on('end', () => {
    const body = Buffer.concat(chunks);
    const headers = { ...req.headers, host: 'localhost' };
    if (token) headers['x-postern-control-token'] = token;
    delete headers['content-length'];
    if (body.length) headers['content-length'] = String(body.length);

    const proxyReq = http.request(
      { socketPath: SOCK, path: req.url, method: req.method, headers },
      (proxyRes) => {
        res.writeHead(proxyRes.statusCode || 502, proxyRes.headers);
        proxyRes.pipe(res);
      },
    );
    proxyReq.on('error', (e) => {
      res.writeHead(502, { 'content-type': 'application/json' });
      res.end(JSON.stringify({ error: { code: 'bridge_unreachable', message: String(e) } }));
    });
    if (body.length) proxyReq.write(body);
    proxyReq.end();
  });
});

server.listen(PORT, '127.0.0.1', () => {
  console.log(`control-bridge: http://127.0.0.1:${PORT} -> ${SOCK} (token ${token ? 'on' : 'off'})`);
});
