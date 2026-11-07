'use strict';

/**
 * End-to-end join check against the Rust server with the exact client the
 * renderer ships (socket.io-client 4.8.3 from the JS reference tree).
 *
 * Covers: DM host key, fresh player identity, rejoin with the secret, wrong
 * secret, playersUpdate on both sides, /api/info and /files/<name>.
 */

const { spawn } = require('child_process');
const fs = require('fs');
const os = require('os');
const path = require('path');

const { io } = require('E:/DnD/node_modules/socket.io-client');

const BIN = 'E:/DnD-rs/target/debug/dnd-server.exe';
const RENDERER = 'E:/DnD-rs/renderer';
const HOST_KEY = 'join-test-host-key';
const TIMEOUT = 20000;

function fail(message) {
  console.error('FAIL:', message);
  process.exit(1);
}

function startServer(dataDir) {
  return new Promise((resolve, reject) => {
    const child = spawn(BIN, [], {
      env: {
        ...process.env,
        DND_DATA: dataDir,
        DND_HOST_KEY: HOST_KEY,
        DND_PORT: '0',
        DND_RENDERER: RENDERER
      },
      stdio: ['ignore', 'pipe', 'pipe']
    });
    let buffer = '';
    const timer = setTimeout(() => reject(new Error('server did not start')), TIMEOUT);
    child.stdout.on('data', (chunk) => {
      buffer += chunk.toString();
      const listen = buffer.match(/LISTENING 127\.0\.0\.1:(\d+)/);
      const campaign = buffer.match(/CAMPAIGN (\S+)/);
      if (listen && campaign) {
        clearTimeout(timer);
        resolve({ child, port: Number(listen[1]), campaignId: campaign[1] });
      }
    });
    child.stderr.on('data', (chunk) => process.stderr.write(chunk));
    child.on('exit', (code) => reject(new Error(`server exited with ${code}`)));
  });
}

function connect(port) {
  return new Promise((resolve, reject) => {
    const socket = io(`http://127.0.0.1:${port}`, {
      transports: ['websocket'],
      reconnection: false,
      timeout: TIMEOUT
    });
    socket.on('connect', () => resolve(socket));
    socket.on('connect_error', reject);
  });
}

function join(socket, payload) {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(`join timeout: ${JSON.stringify(payload)}`)), TIMEOUT);
    let joined = null;
    socket.once('joined', (event) => {
      joined = event;
    });
    socket.emit('joinRoom', payload, (response) => {
      clearTimeout(timer);
      resolve({ response, joined });
    });
  });
}

(async () => {
  const dataDir = fs.mkdtempSync(path.join(os.tmpdir(), 'dnd-join-'));
  const { child, port, campaignId } = await startServer(dataDir);
  const sockets = [];
  try {
    // DM
    const dm = await connect(port);
    sockets.push(dm);
    let updates = 0;
    dm.on('playersUpdate', () => { updates += 1; });
    const dmJoin = await join(dm, { name: 'Мастер', hostKey: HOST_KEY });
    if (!dmJoin.response.ok) fail(`dm join failed: ${JSON.stringify(dmJoin.response)}`);
    if (dmJoin.response.you.role !== 'dm') fail('dm got the wrong role');
    if (!dmJoin.joined || dmJoin.joined.state.campaign.id !== campaignId) fail('dm snapshot has no campaign');
    if (dmJoin.joined.state.players.length !== 1) fail('dm snapshot has no dm player');
    if (dmJoin.joined.state.players[0].avatar !== 'avatar-dm.png') fail('dm avatar missing');

    // Player with a fresh identity
    const player = await connect(port);
    sockets.push(player);
    let playerUpdates = 0;
    player.on('playersUpdate', () => { playerUpdates += 1; });
    const first = await join(player, { name: 'Герой' });
    if (!first.response.ok) fail(`player join failed: ${JSON.stringify(first.response)}`);
    const you = first.response.you;
    if (you.role !== 'player') fail('player got the wrong role');
    if (!you.playerId.startsWith('p-')) fail(`player id not generated: ${you.playerId}`);
    if (you.playerKey.length !== 48) fail(`player key length ${you.playerKey.length}`);
    if (!first.joined.state.sheets[you.playerId]) fail('new player has no empty sheet');
    if (first.joined.state.journal !== '') fail('player sees the DM journal');

    await new Promise((resolve) => setTimeout(resolve, 300));
    if (updates < 1) fail('dm never got playersUpdate');
    if (playerUpdates < 1) fail('player never got playersUpdate');

    // Rejoin with the same id and secret
    const again = await connect(port);
    sockets.push(again);
    const second = await join(again, { name: 'Герой', playerId: you.playerId, playerKey: you.playerKey });
    if (!second.response.ok) fail(`rejoin failed: ${JSON.stringify(second.response)}`);
    if (second.response.you.playerId !== you.playerId) fail('rejoin changed the player id');

    // Wrong secret must be refused
    const thief = await connect(port);
    sockets.push(thief);
    const denied = await join(thief, { name: 'Вор', playerId: you.playerId, playerKey: 'wrong' });
    if (denied.response.ok) fail('wrong playerKey was accepted');
    if (denied.response.code !== 'identity-denied') fail('wrong denial code');

    // HTTP surface
    const info = await (await fetch(`http://127.0.0.1:${port}/api/info`)).json();
    if (!info.ok || info.players < 2) fail(`api/info players=${info.players}`);
    const rootText = await (await fetch(`http://127.0.0.1:${port}/`)).text();
    if (!rootText.includes('работает')) fail('root text missing');

    const filesDir = path.join(dataDir, 'campaigns', campaignId, 'files');
    fs.mkdirSync(filesDir, { recursive: true });
    fs.writeFileSync(path.join(filesDir, 'test.png'), Buffer.from('89504e470d0a1a0a', 'hex'));
    const file = await fetch(`http://127.0.0.1:${port}/files/test.png`);
    if (file.status !== 200) fail(`file status ${file.status}`);
    if (file.headers.get('content-type') !== 'image/png') fail('file content type');
    const index = await fetch(`http://127.0.0.1:${port}/index.html`);
    if (index.status !== 200) fail('renderer not served');

    console.log('PASS dm join', dmJoin.response.you.playerId);
    console.log('PASS player join', you.playerId, 'key', you.playerKey.length);
    console.log('PASS rejoin, identity denial, playersUpdate x2');
    console.log('PASS /api/info, /files/test.png, /index.html');
  } finally {
    for (const socket of sockets) socket.close();
    child.kill();
    fs.rmSync(dataDir, { recursive: true, force: true });
  }
})().catch((error) => fail(error.stack || error.message));
