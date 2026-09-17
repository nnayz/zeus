import https from 'node:https';
import http from 'node:http';
import fs from 'node:fs';
import net from 'node:net';
import { spawnSync } from 'node:child_process';

const certPath = process.env.CERT_PATH;
const keyPath = process.env.KEY_PATH;
const pairPath = process.env.PAIR_PATH;
const pairHttpsPath = process.env.PAIR_HTTPS_PATH;
const listenPort = Number(process.env.LISTEN_PORT || 8443);

if (!certPath || !keyPath || !pairPath || !pairHttpsPath) {
  console.error('Missing required environment variables (CERT_PATH, KEY_PATH, PAIR_PATH, PAIR_HTTPS_PATH).');
  process.exit(1);
}

// Wait for fixture to write pair.json
const deadline = Date.now() + 20_000;
while (!fs.existsSync(pairPath)) {
  if (Date.now() >= deadline) {
    console.error('Timed out waiting for fixture to generate pair.json');
    process.exit(1);
  }
  await new Promise(resolve => setTimeout(resolve, 100));
}

let pairData;
try {
  pairData = JSON.parse(fs.readFileSync(pairPath, 'utf8'));
} catch (err) {
  console.error('Failed to read pair.json:', err.message);
  process.exit(1);
}

const targetUrl = new URL(pairData.origin);
const httpsData = { ...pairData, origin: `https://localhost:${listenPort}` };
const httpsPayload = JSON.stringify(httpsData);

fs.writeFileSync(pairHttpsPath, httpsPayload, { mode: 0o600 });
fs.chmodSync(pairHttpsPath, 0o600);

// Copy to macOS clipboard if pbcopy is available
let copiedToClipboard = false;
if (process.platform === 'darwin') {
  const copied = spawnSync('pbcopy', [], { input: httpsPayload });
  copiedToClipboard = copied.status === 0;
}

const options = {
  cert: fs.readFileSync(certPath),
  key: fs.readFileSync(keyPath)
};

const server = https.createServer(options, (req, res) => {
  const proxyReq = http.request({
    host: targetUrl.hostname,
    port: targetUrl.port,
    path: req.url,
    method: req.method,
    headers: { ...req.headers, host: targetUrl.host }
  }, proxyRes => {
    res.writeHead(proxyRes.statusCode, proxyRes.headers);
    proxyRes.pipe(res);
  });
  proxyReq.on('error', error => {
    res.writeHead(502);
    res.end(`Proxy Error: ${error.message}`);
  });
  req.pipe(proxyReq);
});

// Proxy WebSocket connections
server.on('upgrade', (req, socket, head) => {
  const proxySocket = net.connect(Number(targetUrl.port), targetUrl.hostname, () => {
    proxySocket.write(`${req.method} ${req.url} HTTP/${req.httpVersion}
`);
    for (let i = 0; i < req.rawHeaders.length; i += 2) {
      const header = req.rawHeaders[i];
      const value = header.toLowerCase() === 'host' ? targetUrl.host : req.rawHeaders[i + 1];
      proxySocket.write(`${header}: ${value}
`);
    }
    proxySocket.write('\r\n');
    if (head.length > 0) proxySocket.write(head);
    socket.pipe(proxySocket);
    proxySocket.pipe(socket);
  });
  proxySocket.on('error', error => socket.destroy(error));
  socket.on('error', error => proxySocket.destroy(error));
});

server.listen(listenPort, () => {
  console.log('\n============================================================');
  console.log(`✓ Zeus Companion HTTPS Gateway ready at https://localhost:${listenPort}`);
  if (copiedToClipboard) {
    console.log('✓ Pairing payload copied to clipboard.');
  } else {
    console.log(`✓ Pairing payload written to ${pairHttpsPath}`);
  }
  console.log('-> In iOS Simulator: paste into "Pair with Zeus" and tap "Pair device".');
  console.log('-> Press Ctrl+C in this terminal when finished.');
  console.log('============================================================\n');
});
