import test from 'node:test';
import assert from 'node:assert/strict';
import { randomBytes, randomUUID } from 'node:crypto';
import { once } from 'node:events';
import { DatabaseSync } from 'node:sqlite';
import { mkdtempSync, rmSync, rmdirSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { openStore } from './store.mjs';
import { createAnalytics, configFromEnv, normalizeIP, verifyTurnstile } from './server.mjs';

const origin = 'https://0nionow1.github.io';
const valid = { success: true, hostname: '0nionow1.github.io', action: 'mukhomor-analytics' };
async function fixture(t, overrides = {}, verify = async () => valid) {
  let clock = Date.UTC(2026, 9, 15, 12);
  const config = { origin, hostname: '0nionow1.github.io', secret: randomBytes(32).toString('hex'), proxyKey: randomBytes(32).toString('hex'), turnstileSecret: 'test-only', dailySessions: 4000, dailyEvents: 20000, ...overrides };
  const store = openStore(':memory:');
  const { server } = createAnalytics(config, { store, now: () => clock, verify });
  server.listen(0, '127.0.0.1'); await once(server, 'listening');
  t.after(async () => { server.closeAllConnections(); await new Promise(resolve => server.close(resolve)); store.close(); });
  const base = `http://127.0.0.1:${server.address().port}`;
  const headers = { Origin: origin, 'Content-Type': 'application/json', 'X-Analytics-Proxy-Key': config.proxyKey, 'X-Analytics-IP': '192.0.2.1', 'X-Analytics-Country': 'DE', 'User-Agent': 'test-browser' };
  const post = (path, body, extra = {}, method = 'POST') => fetch(base + path, { method, headers: { ...headers, ...extra }, ...(method === 'OPTIONS' || method === 'GET' ? {} : { body: typeof body === 'string' ? body : JSON.stringify(body) }) });
  const issue = async (body = {}, extra = {}) => {
    const response = await post('/v1/session', { challenge: 'test-challenge', ...body }, extra);
    assert.equal(response.status, 200, await response.clone().text()); return response.json();
  };
  const event = (session, kind = 'page_view', id = randomUUID(), extra = {}, body = {}) => post('/v1/event', { id, kind, page: 'home', ...body }, { Authorization: `Bearer ${session.token}`, ...extra });
  return { config, store, post, issue, event, headers, time: () => clock, advance: ms => { clock += ms; } };
}

test('authenticated visits, daily distinct visitors, countries and download subset', async t => {
  const f = await fixture(t), first = await f.issue();
  assert.equal((await f.event(first, 'github')).status, 409);
  const id = randomUUID();
  assert.equal((await f.event(first, 'page_view', id)).status, 204);
  assert.equal((await f.event(first, 'page_view', id)).status, 204);
  assert.equal((await f.event(first, 'page_view')).status, 204);
  assert.equal((await f.event(first, 'github')).status, 204);
  assert.equal((await f.event(first, 'download')).status, 204);
  let report = f.store.report(30, f.time());
  assert.deepEqual([report.uniqueVisitors, report.visits, report.pageviews, report.github, report.downloads], [1, 1, 2, 2, 1]);
  f.advance(86400000);
  const second = await f.issue({ visitorToken: first.visitorToken }, { 'X-Analytics-Country': 'FR' });
  assert.equal((await f.event(second)).status, 204);
  report = f.store.report(30, f.time());
  assert.equal(report.uniqueVisitors, 1); assert.equal(report.visits, 2);
  assert.equal(report.daily.length, 2); assert.equal(report.countries.length, 2);
  assert.equal(report.countries[0].uniqueVisitors, 1);
});

test('reject forged token, changed IP, UA, origin, proxy and expiry', async t => {
  const f = await fixture(t), session = await f.issue();
  for (const extra of [
    { Authorization: `Bearer ${session.token.slice(0, -1)}!` },
    { 'X-Analytics-IP': '192.0.2.2' }, { 'User-Agent': 'other' }
  ]) assert.equal((await f.event(session, 'page_view', randomUUID(), extra)).status, 401);
  for (const extra of [{ Origin: 'https://attacker.example' }, { Origin: '' }, { 'X-Analytics-Proxy-Key': 'spoof' }, { 'X-Analytics-IP': 'garbage' }]) {
    assert.equal((await f.event(session, 'page_view', randomUUID(), extra)).status, 403);
  }
  f.advance(30 * 60000);
  assert.equal((await f.event(session)).status, 401);
  assert.equal(f.store.report(30, f.time()).visits, 0);
});

test('Turnstile is checked for success, hostname and action; replay rejected by verifier', async t => {
  const used = new Set();
  const f = await fixture(t, {}, async ({ token }) => {
    if (used.has(token)) return { success: false };
    used.add(token);
    if (token === 'wrong-host') return { ...valid, hostname: 'attacker.example' };
    if (token === 'wrong-action') return { ...valid, action: 'other' };
    return valid;
  });
  await f.issue();
  for (const challenge of ['test-challenge', 'wrong-host', 'wrong-action']) {
    assert.equal((await f.post('/v1/session', { challenge })).status, 403);
  }
});

test('country comes from trusted proxy; payload cannot set it; no raw IP in reports', async t => {
  const f = await fixture(t), session = await f.issue({}, { 'CF-IPCountry': 'US', 'X-Forwarded-For': '203.0.113.9' });
  assert.equal((await f.event(session, 'page_view', randomUUID(), {}, { country: 'US' })).status, 400);
  assert.equal((await f.event(session)).status, 204);
  const report = f.store.report(30, f.time());
  assert.equal(report.countries[0].country, 'DE');
  assert(!JSON.stringify(report).includes('192.0.2.1'));
  const unknown = await f.issue({}, { 'X-Analytics-Country': 'XX' });
  await f.event(unknown);
  assert(f.store.report(30, f.time()).countries.some(row => row.country === 'ZZ'));
});

test('strict payload, HTTP methods, routes, CORS, and bounded body', async t => {
  const f = await fixture(t);
  assert.equal((await f.post('/v1/session', {})).status, 400);
  assert.equal((await f.post('/v1/session?secret=x', {})).status, 404);
  assert.equal((await f.post('/admin', {}, {}, 'GET')).status, 404);
  assert.equal((await f.post('/v1/session', {}, {}, 'GET')).status, 405);
  assert.equal((await f.post('/v1/session', '{')).status, 400);
  assert.equal((await f.post('/v1/session', 'x'.repeat(5000))).status, 413);
  assert.equal((await f.post('/v1/session', {}, { 'Content-Type': 'text/plain' })).status, 415);
  assert.equal((await f.post('/v1/session', {}, { 'Content-Encoding': 'gzip' })).status, 415);
  const cors = await f.post('/v1/event', {}, { 'Access-Control-Request-Method': 'POST', 'Access-Control-Request-Headers': 'authorization,content-type' }, 'OPTIONS');
  assert.equal(cors.status, 204); assert.equal(cors.headers.get('Access-Control-Allow-Origin'), origin);
  assert.equal(cors.headers.get('Access-Control-Allow-Credentials'), null);
  assert.equal((await f.post('/v1/event', {}, { 'Access-Control-Request-Method': 'POST', 'Access-Control-Request-Headers': 'X-Analytics-IP' }, 'OPTIONS')).status, 403);
  const session = await f.issue();
  for (const change of [{ kind: 'arbitrary' }, { page: '/?token=secret' }, { id: 'arbitrary' }]) assert.equal((await f.event(session, 'page_view', randomUUID(), {}, change)).status, 400);
});

test('IP issue limits, per-session click limits and durable daily budget', async t => {
  const f = await fixture(t, { dailyEvents: 12 });
  const session = await f.issue(); await f.event(session);
  for (let i = 0; i < 10; i++) assert.equal((await f.event(session, 'github')).status, 204);
  assert.equal((await f.event(session, 'download')).status, 429);
  assert.equal((await f.event(session)).status, 204);
  assert.equal((await f.event(session)).status, 503);
  for (let i = 0; i < 3; i++) await f.issue();
  assert.equal((await f.post('/v1/session', { challenge: 'new' })).status, 429);
  assert.equal(f.store.report(30, f.time()).github, 10);
});

test('daily sessions bounded even across different IPs', async t => {
  const f = await fixture(t, { dailySessions: 1 });
  await f.issue();
  assert.equal((await f.post('/v1/session', { challenge: 'new' }, { 'X-Analytics-IP': '192.0.2.2' })).status, 503);
  f.advance(86400000); await f.issue();
});

test('IPv6 canonicalisation and /64 quotas, IPv4-mapped equivalence', async t => {
  assert.deepEqual(normalizeIP('::ffff:192.0.2.1'), normalizeIP('192.0.2.1'));
  assert.deepEqual(normalizeIP('2001:db8:0:1::1'), normalizeIP('2001:0db8:0000:0001:0:0:0:1'));
  assert.equal(normalizeIP('2001:db8:0:1::2').network, normalizeIP('2001:db8:0:1::1').network);
  const f = await fixture(t);
  for (let i = 1; i <= 4; i++) await f.issue({}, { 'X-Analytics-IP': `2001:db8::${i}` });
  assert.equal((await f.post('/v1/session', { challenge: 'new' }, { 'X-Analytics-IP': '2001:db8::5' })).status, 429);
});

test('database restart preserves quota and deduplication; retention deletes actual rows', () => {
  const directory = mkdtempSync(join(tmpdir(), 'mukhomor-analytics-test-'));
  const file = join(directory, 'test.sqlite');
  let store, inspect;
  const now = Date.UTC(2026, 9, 15, 12), session = { id: randomUUID(), visitor: 'hashed-test-id', country: 'DE', expires: now + 1800000 };
  const event = { id: randomUUID(), kind: 'page_view', page: 'home' };
  try {
    store = openStore(file);
    assert.equal(store.addSession(session, now, 1), true);
    assert.equal(store.record(session.id, event, now, 1), 204);
    store.close(); store = openStore(file);
    assert.equal(store.addSession({ ...session, id: randomUUID() }, now, 1), false);
    assert.equal(store.record(session.id, event, now, 1), 204);
    assert.equal(store.record(session.id, { ...event, id: randomUUID() }, now, 1), 503);
    assert.equal(store.report(30, now).visits, 1);
    inspect = new DatabaseSync(file, { readOnly: true });
    const count = table => inspect.prepare(`SELECT count(*) AS n FROM ${table}`).get().n;
    assert.equal(count('visitors'), 1);
    store.cleanup(now + 1800000);
    assert.equal(count('sessions'), 0); assert.equal(count('events'), 0);
    store.cleanup(now + 35 * 86400000);
    assert.equal(count('visitors'), 0); assert.equal(count('totals'), 1);
    store.cleanup(now + 395 * 86400000);
    assert.equal(count('totals'), 0);
  } finally {
    inspect?.close(); store?.close();
    for (const suffix of ['', '-wal', '-shm']) rmSync(file + suffix, { force: true });
    rmdirSync(directory);
  }
});

test('request flood is bounded before processing; verification errors fail closed', async t => {
  const f = await fixture(t, {}, async () => { throw new Error('Network failure'); });
  assert.equal((await f.post('/v1/session', { challenge: 'token' })).status, 503);
  for (let i = 0; i < 119; i++) assert.equal((await f.post('/unknown', {})).status, 404);
  assert.equal((await f.post('/unknown', {})).status, 429);
  assert.equal(f.store.report(30, f.time()).visits, 0);
});

test('verification concurrency is capped before external requests', async t => {
  let count = 0, release, started;
  const gate = new Promise(resolve => { release = resolve; });
  const ready = new Promise(resolve => { started = resolve; });
  const f = await fixture(t, {}, async () => { count++; if (count === 8) started(); await gate; return valid; });
  const requests = Array.from({ length: 8 }, (_, i) => f.post('/v1/session', { challenge: 'token' }, { 'X-Analytics-IP': `192.0.2.${i + 1}` }));
  try {
    await ready;
    assert.equal((await f.post('/v1/session', { challenge: 'token' }, { 'X-Analytics-IP': '192.0.2.100' })).status, 503);
    assert.equal(count, 8);
  } finally { release(); }
  for (const response of await Promise.all(requests)) assert.equal(response.status, 200);
});

test('production configuration rejects placeholders and local browser secrets; verifier uses server-side request', async () => {
  assert.throws(() => configFromEnv({}));
  const result = await verifyTurnstile({ token: 'challenge', ip: '192.0.2.1', secret: 'test-only' }, async (url, options) => {
    assert.equal(url, 'https://challenges.cloudflare.com/turnstile/v0/siteverify');
    assert.equal(options.body.get('secret'), 'test-only'); assert.equal(options.body.get('remoteip'), '192.0.2.1');
    assert.equal(options.redirect, 'error'); return { ok: true, json: async () => valid };
  });
  assert.deepEqual(result, valid);
});
