// Visual check: opens the real table, applies a theme, saves a PNG.
// Usage: node tools/ui-shot.js [theme] [outfile]

const { spawn } = require('child_process');
const fs = require('fs');
const os = require('os');
const path = require('path');
const WebSocket = require('E:/DnD/node_modules/ws');

const EXE = process.env.DND_DESKTOP_EXE || path.resolve(__dirname, '..', 'target', 'release', 'dnd-desktop.exe');
const PORT = 9224;
const theme = process.argv[2] || 'traktir';
const out = path.resolve(process.argv[3] || path.join(__dirname, '..', 'target', `ui-${theme}.png`));
const dataDir = fs.mkdtempSync(path.join(os.tmpdir(), 'dnd-ui-'));
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

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

async function evaluate(client, expression) {
  const result = await client.send('Runtime.evaluate', {
    expression,
    awaitPromise: true,
    returnByValue: true
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
    for (let i = 0; i < 60; i += 1) {
      const ready = await evaluate(
        client,
        `typeof window.desktop !== 'undefined' && typeof window.DnDTheme !== 'undefined'`
      );
      if (ready) break;
      await sleep(250);
    }

    await evaluate(client, `(async () => {
      DnDTheme.apply('${theme}');
      document.querySelector('#btn-go-create').click();
    })()`);
    await sleep(400);
    await evaluate(client, `(async () => {
      const input = document.querySelector('#create-name');
      input.value = 'Визуальный тест';
      input.dispatchEvent(new Event('input', { bubbles: true }));
      document.querySelector('#btn-create-start').click();
    })()`);

    for (let i = 0; i < 80; i += 1) {
      const state = await evaluate(client, `({
        table: document.querySelector('#screen-table').classList.contains('is-active'),
        create: document.querySelector('#screen-create').classList.contains('is-active'),
        status: document.querySelector('#create-status').textContent
      })`);
      if (state.table) break;
      if (i === 40) console.log('waiting:', JSON.stringify(state));
      await sleep(250);
    }
    // Give socket join, panels and fonts a moment to settle.
    await sleep(2500);
    const actionErrors = await evaluate(client, `(() => {
      const errors = [];
      try {
        document.querySelectorAll('.col--left > .panel').forEach((panel) => panel.classList.remove('is-collapsed'));
      } catch (error) { errors.push('panels: ' + error.message); }
      try {
        const adventure = document.querySelector('#tabbtn-adventure');
        if (adventure && !adventure.hidden) adventure.click();
      } catch (error) { errors.push('adventure: ' + error.message); }
      try {
        const sound = document.querySelector('.map-sound__toggle');
        if (sound) sound.click();
        else errors.push('sound: no toggle');
      } catch (error) { errors.push('sound: ' + error.message); }
      try { DnDTheme.apply('${theme}'); } catch (error) { errors.push('theme: ' + error.message); }
      return errors;
    })()`);
    if (actionErrors.length) console.log('ui actions:', actionErrors);
    await sleep(700);

    const shot = await client.send('Page.captureScreenshot', { format: 'png' });
    fs.writeFileSync(out, Buffer.from(shot.data, 'base64'));
    console.log(out);
  } finally {
    if (client) client.close();
    app.kill();
    await sleep(500);
    fs.rmSync(dataDir, { recursive: true, force: true });
  }
})().catch((error) => {
  console.error('FAIL', error.message);
  process.exitCode = 1;
});
