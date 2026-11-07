'use strict';

/**
 * Protocol spike: the Rust server must speak Socket.IO to the exact client the
 * renderer ships (socket.io-client 4.8.3 from the JS reference tree).
 *
 * Checks:
 *   1. static renderer is served
 *   2. websocket transport connects and answers `ping` with `pong`
 *   3. polling transport does the same
 */

const { spawn } = require('child_process');

const CLIENT = 'E:/DnD/node_modules/socket.io-client';
const BIN = 'E:/DnD-rs/target/debug/dnd-server.exe';
const RENDERER = 'E:/DnD-rs/renderer';
const { io } = require(CLIENT);

const TIMEOUT_MS = 20000;

function fail(message) {
  console.error('FAIL:', message);
  process.exit(1);
}

function connectServer() {
  return new Promise((resolve, reject) => {
    const child = spawn(BIN, [], {
      env: { ...process.env, DND_PORT: '0', DND_RENDERER: RENDERER },
      stdio: ['ignore', 'pipe', 'pipe']
    });
    let buffer = '';
    const timer = setTimeout(() => reject(new Error('server did not start')), TIMEOUT_MS);
    child.stdout.on('data', (chunk) => {
      buffer += chunk.toString();
      const match = buffer.match(/LISTENING (127\.0\.0\.1:\d+)/);
      if (match) {
        clearTimeout(timer);
        resolve({ child, port: Number(match[1].split(':')[1]) });
      }
    });
    child.stderr.on('data', (chunk) => process.stderr.write(chunk));
    child.on('exit', (code) => reject(new Error(`server exited with ${code}`)));
  });
}

function ping(port, transport) {
  return new Promise((resolve, reject) => {
    const socket = io(`http://127.0.0.1:${port}`, {
      transports: [transport],
      reconnection: false,
      timeout: TIMEOUT_MS
    });
    const timer = setTimeout(() => {
      socket.close();
      reject(new Error(`${transport}: no pong`));
    }, TIMEOUT_MS);
    socket.on('connect_error', (error) => {
      clearTimeout(timer);
      reject(new Error(`${transport}: ${error.message}`));
    });
    socket.on('connect', () => {
      socket.emit('ping', { echo: transport });
    });
    socket.on('pong', (payload) => {
      clearTimeout(timer);
      socket.close();
      if (payload.echo !== transport) reject(new Error(`${transport}: bad echo`));
      else if (typeof payload.time !== 'number') reject(new Error(`${transport}: bad time`));
      else resolve(payload);
    });
  });
}

(async () => {
  const { child, port } = await connectServer();
  try {
    const response = await fetch(`http://127.0.0.1:${port}/index.html`);
    if (response.status !== 200) fail(`index.html status ${response.status}`);
    const html = await response.text();
    if (!html.includes('D&amp;D')) fail('index.html does not look like the renderer');

    const viaWebsocket = await ping(port, 'websocket');
    const viaPolling = await ping(port, 'polling');
    console.log('PASS static renderer', html.length, 'bytes');
    console.log('PASS websocket pong', viaWebsocket);
    console.log('PASS polling pong', viaPolling);
  } finally {
    child.kill();
  }
})().catch((error) => fail(error.message));
