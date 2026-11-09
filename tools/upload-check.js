// Backend check for heavy sound uploads: starts dnd-server, joins as DM,
// uploads 3 MB and 25 MB WAV payloads and verifies the soundboard update.

const { spawn } = require('child_process');
const fs = require('fs');
const os = require('os');
const path = require('path');
const { io } = require('E:/DnD/node_modules/socket.io-client');

const EXE = path.resolve(__dirname, '..', 'target', 'debug', 'dnd-server.exe');
const dataDir = fs.mkdtempSync(path.join(os.tmpdir(), 'dnd-upload-'));

function wavBytes(size) {
  const dataSize = Math.max(1, size - 44);
  const buffer = Buffer.alloc(44 + dataSize, 128);
  buffer.write('RIFF', 0);
  buffer.writeUInt32LE(36 + dataSize, 4);
  buffer.write('WAVE', 8);
  buffer.write('fmt ', 12);
  buffer.writeUInt32LE(16, 16);
  buffer.writeUInt16LE(1, 20);
  buffer.writeUInt16LE(1, 22);
  buffer.writeUInt32LE(8000, 24);
  buffer.writeUInt32LE(8000, 28);
  buffer.writeUInt16LE(1, 32);
  buffer.writeUInt16LE(8, 34);
  buffer.write('data', 36);
  buffer.writeUInt32LE(dataSize, 40);
  return buffer;
}

function waitForServer(child) {
  return new Promise((resolve, reject) => {
    let output = '';
    const timer = setTimeout(() => reject(new Error(`server timeout: ${output}`)), 15000);
    child.stdout.on('data', (chunk) => {
      output += chunk.toString();
      const listening = output.match(/LISTENING ([^\r\n]+)/);
      const key = output.match(/HOSTKEY ([0-9a-f]+)/);
      if (listening && key) {
        clearTimeout(timer);
        resolve({ url: `http://${listening[1]}`, hostKey: key[1] });
      }
    });
    child.stderr.on('data', (chunk) => { output += chunk.toString(); });
    child.once('exit', (code) => reject(new Error(`server exited ${code}: ${output}`)));
  });
}

function join(url, hostKey) {
  return new Promise((resolve, reject) => {
    const socket = io(url, { transports: ['websocket'], reconnection: false });
    socket.on('connect', () => {
      socket.emit('joinRoom', { name: 'Мастер', hostKey }, (ack) => {
        if (ack && ack.ok) resolve(socket);
        else reject(new Error(JSON.stringify(ack)));
      });
    });
    socket.on('connect_error', reject);
  });
}

function upload(socket, name, dataUrl) {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(`upload timeout: ${name}`)), 60000);
    const done = (items) => {
      clearTimeout(timer);
      socket.off('soundboardUpdated', done);
      socket.off('errorMessage', onError);
      resolve(items);
    };
    const onError = (payload) => {
      clearTimeout(timer);
      socket.off('soundboardUpdated', done);
      socket.off('errorMessage', onError);
      reject(new Error(payload && payload.message));
    };
    socket.once('soundboardUpdated', done);
    socket.once('errorMessage', onError);
    socket.emit('uploadSound', { name, sound: dataUrl });
  });
}

(async () => {
  const child = spawn(EXE, [], {
    env: { ...process.env, DND_DATA: dataDir, DND_PORT: '0', DND_RENDERER: path.resolve(__dirname, '..', 'renderer') },
    stdio: ['ignore', 'pipe', 'pipe']
  });
  let socket;
  try {
    const { url, hostKey } = await waitForServer(child);
    socket = await join(url, hostKey);
    socket.on('disconnect', (reason) => console.log('disconnect:', reason));
    const sizes = process.argv.slice(2).map(Number).filter((value) => value > 0);
    for (const megabytes of (sizes.length ? sizes : [3, 10, 20, 25])) {
      const bytes = megabytes * 1024 * 1024;
      const dataUrl = `data:audio/wav;base64,${wavBytes(bytes).toString('base64')}`;
      console.log(`sending ${megabytes} MB (${(dataUrl.length / 1024 / 1024).toFixed(1)} MB base64)`);
      const started = Date.now();
      const items = await upload(socket, `wav-${megabytes}mb`, dataUrl);
      const found = items.find((item) => item.name === `wav-${megabytes}mb`);
      if (!found) throw new Error(`sound ${megabytes} MB not in soundboard`);
      console.log(`PASS ${megabytes} MB upload in ${Date.now() - started} ms, ref ${found.ref}`);
    }

    const formats = [
      ['flac', Buffer.concat([Buffer.from('fLaC'), Buffer.alloc(256)])],
      ['m4a', Buffer.concat([Buffer.from([0, 0, 0, 0x20]), Buffer.from('ftypM4A '), Buffer.alloc(256)])],
      ['webm', Buffer.concat([Buffer.from([0x1a, 0x45, 0xdf, 0xa3]), Buffer.alloc(256)])]
    ];
    const mime = { flac: 'audio/flac', m4a: 'audio/mp4', webm: 'audio/webm' };
    for (const [format, bytes] of formats) {
      const dataUrl = `data:${mime[format]};base64,${bytes.toString('base64')}`;
      const items = await upload(socket, `format-${format}`, dataUrl);
      const found = items.find((item) => item.name === `format-${format}`);
      if (!found || !found.ref.endsWith(`.${format}`)) {
        throw new Error(`format ${format} not accepted`);
      }
      console.log(`PASS ${format} upload, ref ${found.ref}`);
    }
  } finally {
    if (socket) socket.close();
    child.kill();
    fs.rmSync(dataDir, { recursive: true, force: true });
  }
})().catch((error) => {
  console.error('FAIL', error.message);
  process.exitCode = 1;
});
