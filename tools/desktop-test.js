// End-to-end smoke test for the Tauri desktop shell.
// Drives the real webview over the WebView2 DevTools protocol, then joins the
// hosted table with the exact socket.io client the renderer uses.

const { spawn } = require('child_process');
const fs = require('fs');
const os = require('os');
const path = require('path');
const WebSocket = require('E:/DnD/node_modules/ws');
const { io } = require('E:/DnD/node_modules/socket.io-client');

const EXE = process.env.DND_DESKTOP_EXE || path.resolve(__dirname, '..', 'target', 'debug', 'dnd-desktop.exe');
const DEBUG_PORT = 9222;
const dataDir = fs.mkdtempSync(path.join(os.tmpdir(), 'dnd-desktop-test-'));

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

async function findPage() {
  for (let i = 0; i < 60; i += 1) {
    try {
      const response = await fetch(`http://127.0.0.1:${DEBUG_PORT}/json/list`);
      const targets = await response.json();
      const page = targets.find((target) => target.type === 'page' && target.webSocketDebuggerUrl);
      if (page) return page;
    } catch {
      // webview not up yet
    }
    await sleep(250);
  }
  throw new Error('webview devtools did not open');
}

function connectCdp(url) {
  const socket = new WebSocket(url);
  let nextId = 1;
  const pending = new Map();
  socket.on('message', (raw) => {
    const message = JSON.parse(raw.toString());
    const entry = pending.get(message.id);
    if (!entry) return;
    pending.delete(message.id);
    if (message.error) entry.reject(new Error(message.error.message));
    else entry.resolve(message.result);
  });
  const ready = new Promise((resolve, reject) => {
    socket.once('open', resolve);
    socket.once('error', reject);
  });
  return {
    ready,
    send(method, params) {
      const id = nextId++;
      socket.send(JSON.stringify({ id, method, params }));
      return new Promise((resolve, reject) => pending.set(id, { resolve, reject }));
    },
    close: () => socket.close()
  };
}

async function evaluate(cdp, expression) {
  const result = await cdp.send('Runtime.evaluate', {
    expression,
    awaitPromise: true,
    returnByValue: true
  });
  if (result.exceptionDetails) {
    throw new Error(result.exceptionDetails.text || 'evaluate failed');
  }
  return result.result.value;
}

function join(url, payload) {
  return new Promise((resolve, reject) => {
    const socket = io(url, { transports: ['websocket'], reconnection: false, timeout: 8000 });
    const timer = setTimeout(() => reject(new Error('join timeout')), 10000);
    socket.on('connect', () => socket.emit('joinRoom', payload, (ack) => {
      clearTimeout(timer);
      resolve({ socket, ack });
    }));
    socket.on('connect_error', (error) => {
      clearTimeout(timer);
      reject(error);
    });
  });
}

(async () => {
  const app = spawn(EXE, [], {
    env: {
      ...process.env,
      DND_DATA: dataDir,
      WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${DEBUG_PORT}`
    },
    stdio: 'ignore'
  });

  let cdp;
  const sockets = [];
  try {
    const page = await findPage();
    cdp = connectCdp(page.webSocketDebuggerUrl);
    await cdp.ready;

    const boot = await evaluate(cdp, `(async () => {
      const config = await desktop.getConfig();
      const created = await desktop.createCampaign('Smoke');
      const started = await desktop.startHost();
      return { config, created, started };
    })()`);

    if (!boot.config || !boot.config.dataDir.includes('dnd-desktop-test-')) {
      throw new Error(`data dir mismatch: ${boot.config && boot.config.dataDir}`);
    }
    console.log('PASS bridge getConfig data dir');
    if (!boot.created.ok) throw new Error(`create failed: ${boot.created.error}`);
    console.log('PASS create campaign in webview');
    if (!boot.started.ok) throw new Error(`start failed: ${boot.started.error}`);
    console.log(`PASS host started on ${boot.started.localUrl} key ${boot.started.hostKey.length} chars`);

    const url = boot.started.localUrl;
    const info = await (await fetch(`${url}/api/info`)).json();
    if (!info.ok || info.campaign.name !== 'Smoke') throw new Error('api/info mismatch');
    console.log('PASS /api/info');
    const pageHtml = await (await fetch(`${url}/index.html`)).text();
    if (!pageHtml.includes('app.js')) throw new Error('renderer not served over HTTP');
    console.log('PASS renderer served to network players');

    const dm = await join(url, { name: 'Мастер', hostKey: boot.started.hostKey });
    sockets.push(dm.socket);
    if (!dm.ack.ok || dm.ack.you.role !== 'dm') throw new Error('dm join failed');

    let updates = 0;
    const updatesSeen = new Promise((resolve) => {
      dm.socket.on('playersUpdate', () => {
        updates += 1;
        if (updates === 2) resolve(updates);
      });
      setTimeout(() => resolve(updates), 4000);
    });

    const player = await join(url, { name: 'Игрок' });
    sockets.push(player.socket);
    if (!player.ack.ok || player.ack.you.role !== 'player') throw new Error('player join failed');
    console.log(`PASS joins: dm=${dm.ack.you.playerId} player=${player.ack.you.playerId}`);

    const received = await updatesSeen;
    if (received < 2) throw new Error(`expected 2 playersUpdate, got ${received}`);
    console.log('PASS playersUpdate reaches DM');

    const stopped = await evaluate(cdp, `(async () => {
      const stop = await desktop.stopHost();
      const status = await desktop.hostStatus();
      return { stop, status };
    })()`);
    if (!stopped.stop.ok || stopped.status.running) throw new Error('stopHost failed');
    console.log('PASS stopHost');

    const cleanup = await evaluate(cdp, `(async () => {
      const current = await desktop.currentCampaign();
      return desktop.deleteCampaign(current.id);
    })()`);
    if (!cleanup.ok) throw new Error('delete failed');
    console.log('PASS delete campaign');
  } finally {
    for (const socket of sockets) socket.close();
    if (cdp) cdp.close();
    app.kill();
    await sleep(300);
    fs.rmSync(dataDir, { recursive: true, force: true });
  }
})().catch((error) => {
  console.error('FAIL', error.message);
  process.exitCode = 1;
});
