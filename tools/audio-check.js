// Targeted audio check: real WebView2, real HTTP file, real <audio>.
// Verifies CORS, play(), loop bookkeeping and table volume.

const { spawn } = require('child_process');
const fs = require('fs');
const os = require('os');
const path = require('path');
const WebSocket = require('E:/DnD/node_modules/ws');

const EXE = process.env.DND_DESKTOP_EXE || path.resolve(__dirname, '..', 'target', 'debug', 'dnd-desktop.exe');
const PORT = 9225;
const dataDir = fs.mkdtempSync(path.join(os.tmpdir(), 'dnd-audio-'));
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

function wav(seconds = 1, rate = 8000) {
  const samples = rate * seconds;
  const data = Buffer.alloc(samples);
  for (let i = 0; i < samples; i += 1) {
    data[i] = 128 + Math.round(50 * Math.sin((2 * Math.PI * 440 * i) / rate));
  }
  const header = Buffer.alloc(44);
  header.write('RIFF', 0);
  header.writeUInt32LE(36 + data.length, 4);
  header.write('WAVE', 8);
  header.write('fmt ', 12);
  header.writeUInt32LE(16, 16);
  header.writeUInt16LE(1, 20);
  header.writeUInt16LE(1, 22);
  header.writeUInt32LE(rate, 24);
  header.writeUInt32LE(rate, 28);
  header.writeUInt16LE(1, 32);
  header.writeUInt16LE(8, 34);
  header.write('data', 36);
  header.writeUInt32LE(data.length, 40);
  return Buffer.concat([header, data]);
}

async function pageTarget() {
  for (let i = 0; i < 80; i += 1) {
    try {
      const list = await (await fetch(`http://127.0.0.1:${PORT}/json/list`)).json();
      const page = list.find((item) => item.type === 'page' && item.webSocketDebuggerUrl);
      if (page) return page;
    } catch {}
    await sleep(250);
  }
  throw new Error('no devtools page');
}

function cdp(url) {
  const socket = new WebSocket(url);
  let id = 1;
  const pending = new Map();
  socket.on('message', (raw) => {
    const message = JSON.parse(raw.toString());
    const entry = pending.get(message.id);
    if (!entry) return;
    pending.delete(message.id);
    if (message.error) entry.reject(new Error(message.error.message));
    else entry.resolve(message.result);
  });
  return {
    ready: new Promise((resolve, reject) => {
      socket.once('open', resolve);
      socket.once('error', reject);
    }),
    send(method, params) {
      const requestId = id++;
      socket.send(JSON.stringify({ id: requestId, method, params }));
      return new Promise((resolve, reject) => pending.set(requestId, { resolve, reject }));
    },
    close: () => socket.close()
  };
}

async function evaluate(client, expression, userGesture = false) {
  const result = await client.send('Runtime.evaluate', {
    expression,
    awaitPromise: true,
    returnByValue: true,
    userGesture
  });
  if (result.exceptionDetails) throw new Error(result.exceptionDetails.text || 'evaluate failed');
  return result.result.value;
}

(async () => {
  const app = spawn(EXE, [], {
    env: {
      ...process.env,
      DND_DATA: dataDir,
      WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${PORT}`
    },
    stdio: 'ignore'
  });
  let client;
  try {
    const page = await pageTarget();
    client = cdp(page.webSocketDebuggerUrl);
    await client.ready;
    for (let i = 0; i < 80; i += 1) {
      if (await evaluate(client, `typeof window.desktop !== 'undefined' && typeof window.DnDSound !== 'undefined'`)) break;
      await sleep(250);
    }

    const started = await evaluate(client, `(async () => {
      const created = await desktop.createCampaign('Audio');
      if (!created.ok) return created;
      const host = await desktop.startHost();
      return { ...host, campaignId: created.campaign.id };
    })()`);
    if (!started.ok) throw new Error(started.error);
    const url = `${started.localUrl}/files/beep.wav`;
    const files = path.join(dataDir, 'campaigns', started.campaignId, 'files');
    fs.mkdirSync(files, { recursive: true });
    fs.writeFileSync(path.join(files, 'beep.wav'), wav(1));

    await evaluate(client, `(() => {
      DnDSound.setEnabled(true);
      window.__played = [];
      const original = HTMLMediaElement.prototype.play;
      HTMLMediaElement.prototype.play = function () {
        window.__played.push(this);
        return original.apply(this, arguments);
      };
      return true;
    })()`);

    await evaluate(client, `DnDSound.tableCue(${JSON.stringify(url)}, 'play')`, true);
    await sleep(1200);

    const played = await evaluate(client, `(() => {
      const el = window.__played[window.__played.length - 1];
      if (!el) return { none: true };
      return {
        crossOrigin: el.crossOrigin,
        paused: el.paused,
        currentTime: el.currentTime,
        duration: el.duration,
        readyState: el.readyState,
        networkState: el.networkState,
        error: el.error ? el.error.code : 0
      };
    })()`);
    console.log('play:', played);
    if (played.none || played.error) throw new Error('audio did not load');
    if (played.crossOrigin !== 'anonymous') throw new Error('crossOrigin not set');

    // Клип длится секунду: к моменту проверки он уже мог дойти до конца.
    // Значит, воспроизведение состоялось, даже если paused === true.
    if (!(played.currentTime > 0)) {
      // Simulate the first real click, then retry the same layer.
      await client.send('Input.dispatchMouseEvent', {
        type: 'mousePressed', x: 20, y: 20, button: 'left', clickCount: 1
      });
      await client.send('Input.dispatchMouseEvent', {
        type: 'mouseReleased', x: 20, y: 20, button: 'left', clickCount: 1
      });
      await evaluate(client, `DnDSound.tableCue(${JSON.stringify(url)}, 'play')`, true);
      await sleep(1200);
      const retried = await evaluate(client, `(() => {
        const el = window.__played[window.__played.length - 1];
        return { paused: el.paused, currentTime: el.currentTime, error: el.error ? el.error.code : 0 };
      })()`);
      console.log('retry:', retried);
      if (!(retried.currentTime > 0)) throw new Error('audio still paused');
    }

    await evaluate(client, `DnDSound.tableCue(${JSON.stringify(url)}, 'loop')`, true);
    await sleep(600);
    const loop = await evaluate(client, `(() => {
      const el = window.__played[window.__played.length - 1];
      return {
        looping: DnDSound.isLooping(${JSON.stringify(url)}),
        refs: DnDSound.loopRefs(),
        elLoop: el ? el.loop : null,
        paused: el ? el.paused : null
      };
    })()`);
    console.log('loop:', loop);
    if (!loop.looping || !loop.elLoop) throw new Error('loop did not start');

    await evaluate(client, `DnDSound.tableCue(${JSON.stringify(url)}, 'stop')`, true);
    await sleep(400);
    const stopped = await evaluate(client, `DnDSound.isLooping(${JSON.stringify(url)})`);
    console.log('stopped:', stopped);
    if (stopped) throw new Error('loop did not stop');

    const volume = await evaluate(client, `(() => {
      DnDSound.setTableVolume(0);
      const zero = DnDSound.tableVolume();
      DnDSound.setTableVolume(80);
      return { zero, back: DnDSound.tableVolume() };
    })()`);
    console.log('volume:', volume);
    if (volume.zero !== 0 || volume.back !== 0.8) throw new Error('volume did not persist');

    await evaluate(client, `(async () => {
      const current = await desktop.currentCampaign();
      await desktop.stopHost();
      return desktop.deleteCampaign(current.id);
    })()`);
    console.log('PASS audio');
  } finally {
    if (client) client.close();
    app.kill();
    await sleep(400);
    fs.rmSync(dataDir, { recursive: true, force: true });
  }
})().catch((error) => {
  console.error('FAIL', error.message);
  process.exitCode = 1;
});
