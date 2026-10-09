const dgram = require('node:dgram');
const net = require('node:net');
const fs = require('node:fs');
const path = require('node:path');
const assert = require('node:assert/strict');
const [root, mode] = process.argv.slice(2);
const settings = JSON.parse(fs.readFileSync(path.join(root, 'settings.json')));
const token = fs.readFileSync(path.join(root, 'private/api-token.txt'), 'utf8').trim();
const sockets = [];
let control;
const sleep = ms => new Promise(r => setTimeout(r, ms));
async function api(route) {
  const r = await fetch(`http://127.0.0.1:${settings.ports.controller}${route}`, {headers: {Authorization: `Bearer ${token}`}});
  assert(r.ok, `Controller HTTP ${r.status}`);
  return r.json();
}
async function echo(family, address) {
  const s = dgram.createSocket(family); sockets.push(s);
  s.on('message', (b, peer) => s.send(b, peer.port, peer.address));
  await new Promise((r, j) => {s.once('error', j); s.bind(0, address, r);});
  return s.address().port;
}
async function associate() {
  control = net.connect(settings.ports.proxy, '127.0.0.1');
  await new Promise((r, j) => {control.once('connect', r); control.once('error', j);});
  const receive = () => new Promise((r, j) => {
    const timer = setTimeout(() => {control.off('data', onData); j(new Error('SOCKS control timeout'));}, 2000);
    const onData = b => {clearTimeout(timer); r(b);};
    control.once('data', onData);
  });
  let pending = receive(); control.write(Buffer.from([5,1,0]));
  assert.deepEqual(await pending, Buffer.from([5,0]));
  pending = receive(); control.write(Buffer.from([5,3,0,1,0,0,0,0,0,0]));
  const reply = await pending; assert.equal(reply[1], 0, 'SOCKS UDP associate failed');
  return reply.readUInt16BE(reply.length - 2);
}
function packet(host, port, body) {
  let target;
  if (net.isIP(host) === 4) target = Buffer.from([1, ...host.split('.').map(Number)]);
  else if (host === '::1') target = Buffer.from([4, ...Array(15).fill(0), 1]);
  else {const name = Buffer.from(host); target = Buffer.concat([Buffer.from([3,name.length]),name]);}
  const p = Buffer.alloc(2); p.writeUInt16BE(port);
  return Buffer.concat([Buffer.alloc(3), target, p, body]);
}
function payload(b) {
  assert.equal(b[2], 0, 'Fragmented SOCKS reply');
  const len = b[3] === 1 ? 4 : b[3] === 4 ? 16 : b[3] === 3 ? b[4] + 1 : -1;
  assert(len >= 0); return b.subarray(4 + len + 2);
}
async function main() {
  const ports = [await echo('udp4','127.0.0.1'), await echo('udp4','127.0.0.1')];
  const v6 = await echo('udp6','::1');
  const relayPort = await associate();
  const client = dgram.createSocket('udp4'); sockets.push(client);
  await new Promise(r => client.bind(0, '0.0.0.0', r));
  let serial = 0;
  async function round(host, port, shouldReply = true) {
    const body = Buffer.alloc(1200, 0x5a); body.writeUInt32BE(++serial);
    const result = new Promise((r,j) => {
      const onMessage = b => {clearTimeout(timer); try {assert.deepEqual(payload(b), body); r(true);} catch(e) {j(e);} };
      const timer = setTimeout(() => {client.off('message', onMessage); r(false);}, shouldReply ? 3000 : 600);
      client.once('message', onMessage);
    });
    client.send(packet(host,port,body),relayPort,'127.0.0.1');
    assert.equal(await result, shouldReply, `UDP route ${mode}: ${host}`);
  }
  if (mode === 'blocked') {
    await round('unrelated.example', ports[0], false);
    await round('127.0.0.1', ports[0], false);
    console.log('PASS: unrelated-domain and bare-IP UDP obey REJECT when EXE bypass is absent');
    return;
  }
  const targets = ['process', 'path', 'ip'].includes(mode)
    ? [['unrelated.example',ports[0]],['127.0.0.1',ports[1]],['::1',v6]]
    : [['allowed.ru',ports[0]],['sub.allowed.ru',ports[1]]];
  for (let n=0;n<200;n++) {
    for (const [host,port] of targets) await round(host,port);
  }
  const connections = (await api('/connections')).connections.filter(c => c.metadata.network === 'udp');
  assert(connections.length, 'Missing live UDP connections');
  assert(connections.every(c => c.chains.includes('DIRECT')), 'UDP unexpectedly went to another policy');
  if (mode === 'process') {
    assert(connections.every(c => c.rule === 'ProcessName' && c.rulePayload.toLowerCase() === 'node.exe'), 'Wildcard UDP EXE attribution failed');
  } else if (mode === 'path') {
    assert(connections.every(c => c.rule === 'ProcessPath' && c.rulePayload.toLowerCase() === process.execPath.toLowerCase()), 'Full EXE path attribution failed');
  } else if (mode === 'ip') {
    assert(connections.every(c => c.rule === 'RuleSet' && c.rulePayload === 'custom-ips'), 'Native IPv4/IPv6 IP set did not match');
  } else assert(connections.every(c => c.rule === 'DomainSuffix' ||
    (c.rule === 'RuleSet' && c.rulePayload === 'custom-domains')), 'Domain UDP rule did not match');
  await sleep(2000);
  for (const [host,port] of targets) await round(host,port);
  console.log(`PASS: ${mode} bypass, ${serial} UDP replies of 1200 bytes, multiple peers/IPv${['process', 'path', 'ip'].includes(mode) ? '4+6' : '4'}, wildcard socket, DIRECT confirmed via API, resumes after idle`);
}
main().catch(e => {console.error(e.message); process.exitCode=1;}).finally(() => {
  if(control) control.destroy(); for(const s of sockets) s.close();
});
