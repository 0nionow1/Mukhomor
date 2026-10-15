import test from 'node:test';
import assert from 'node:assert/strict';
import { randomBytes, randomUUID } from 'node:crypto';
import { once } from 'node:events';
import { mkdtempSync, rmSync, rmdirSync, existsSync, readFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { openStore } from './store.mjs';
import { createAnalytics } from './server.mjs';
import { hashPassword, adminConfigFromEnv } from './admin.mjs';

const origin = 'https://stats.example.test:10443';
const password = 'test-only-password-42';
const passwordHash = await hashPassword(password);
const telegram = { token: '123456:' + 'a'.repeat(35), chatId: '1234567' };
async function fixture(t, overrides = {}, delivery) {
  let clock = Date.UTC(2026, 9, 15, 12);
  const config = { origin: 'https://0nionow1.github.io', hostname: '0nionow1.github.io', secret: randomBytes(32).toString('hex'), proxyKey: randomBytes(32).toString('hex'), dailySessions: 4000, dailyEvents: 20000, admin: { origin, username: 'owner', passwordHash, telegram }, ...overrides };
  const store = openStore(':memory:');
  const delivered = [];
  const { server } = createAnalytics(config, { store, now: () => clock, verify: async () => ({ success: true, hostname: config.hostname, action: 'mukhomor-analytics' }), sendAdminCode: async data => { delivered.push(data); if (delivery) await delivery(data); } });
  server.listen(0, '127.0.0.1'); await once(server, 'listening');
  t.after(async () => { server.closeAllConnections(); await new Promise(resolve => server.close(resolve)); store.close(); });
  const base = `http://127.0.0.1:${server.address().port}`;
  const headers = { Origin: origin, 'Content-Type': 'application/json', 'X-Admin-Request': '1', 'X-Analytics-Proxy-Key': config.proxyKey, 'X-Analytics-IP': '192.0.2.1', 'X-Analytics-Proto': 'https', 'X-Analytics-Host': new URL(origin).host, 'User-Agent': 'test-browser', 'Sec-Fetch-Site': 'same-origin' };
  const call = (path, { method = 'GET', body, extra = {} } = {}) => fetch(base + path, { method, redirect: 'manual', headers: { ...headers, ...extra }, ...(body === undefined ? {} : { body: typeof body === 'string' ? body : JSON.stringify(body) }) });
  const login = (body = { username: 'owner', password }, extra = {}) => call('/admin/api/login', { method: 'POST', body, extra });
  const challenge = async (extra = {}) => {
    const response = await login(undefined, extra); assert.equal(response.status, 202, await response.clone().text());
    return { cookie: response.headers.get('set-cookie').split(';')[0], code: delivered.at(-1).code, ...await response.json() };
  };
  const verify = (pending, code = pending.code, extra = {}) => call('/admin/api/verify', { method: 'POST', body: { code }, extra: { Cookie: pending.cookie, ...extra } });
  const auth = async () => {
    const response = await verify(await challenge()); assert.equal(response.status, 200, await response.clone().text());
    return { cookie: response.headers.get('set-cookie').split(';')[0], ...await response.json() };
  };
  return { config, store, call, login, auth, challenge, verify, delivered, time: () => clock, advance: ms => { clock += ms; } };
}

test('admin disabled by default; production config requires complete HTTPS settings and scrypt', async t => {
  assert.equal(adminConfigFromEnv({}), null);
  for (const env of [
    { ADMIN_USERNAME: 'owner' },
    { ADMIN_ORIGIN: 'http://stats.example.test', ADMIN_USERNAME: 'owner', ADMIN_PASSWORD_HASH: passwordHash },
    { ADMIN_ORIGIN: origin + '/subpath', ADMIN_USERNAME: 'owner', ADMIN_PASSWORD_HASH: passwordHash },
    { ADMIN_ORIGIN: origin, ADMIN_USERNAME: 'owner', ADMIN_PASSWORD_HASH: password }
  ]) assert.throws(() => adminConfigFromEnv(env));
  assert.throws(() => adminConfigFromEnv({ ADMIN_ORIGIN: origin, ADMIN_USERNAME: 'owner', ADMIN_PASSWORD_HASH: passwordHash }));
  assert.equal(adminConfigFromEnv({ ADMIN_ORIGIN: origin, ADMIN_USERNAME: 'owner', ADMIN_PASSWORD_HASH: passwordHash, TELEGRAM_BOT_TOKEN: telegram.token, TELEGRAM_CHAT_ID: telegram.chatId }).origin, origin);
  const f = await fixture(t, { admin: null }); assert.equal((await f.call('/admin/')).status, 404);
});

test('login issues secure cookie; protected report uses actual counts; no secret in JSON or cache', async t => {
  const f = await fixture(t);
  for (const path of ['/admin/api/session', '/admin/api/report?days=7']) assert.equal((await f.call(path)).status, 401);
  const id = randomUUID(); f.store.addSession({ id, visitor: 'visitor', country: 'DE', expires: f.time() + 60000 }, f.time(), 4000);
  for (const kind of ['page_view', 'download']) f.store.record(id, { id: randomUUID(), kind }, f.time(), 20000);
  const pending = await f.challenge();
  assert.equal((await f.call('/admin/api/report?days=7', { extra: { Cookie: pending.cookie } })).status, 401);
  const response = await f.verify(pending), header = response.headers.get('set-cookie');
  assert.equal(response.status, 200);
  for (const flag of ['HttpOnly', 'Secure', 'SameSite=Strict', 'Path=/', 'Max-Age=3600']) assert.ok(header.includes(flag));
  assert.ok(!header.includes('Domain=')); assert.ok(header.startsWith('__Host-'));
  const body = await response.json(); assert.deepEqual(Object.keys(body).sort(), ['csrf', 'expires', 'idle', 'username']);
  assert.equal(JSON.stringify(body).includes(password), false); assert.equal(JSON.stringify(body).includes(passwordHash), false);
  const report = await f.call('/admin/api/report?days=7', { extra: { Cookie: header.split(';')[0] } });
  assert.equal(report.status, 200); assert.equal(report.headers.get('cache-control'), 'no-store');
  assert.equal(report.headers.get('access-control-allow-origin'), null);
  assert.match(report.headers.get('content-security-policy'), /frame-ancestors 'none'/);
  assert.match(report.headers.get('x-robots-tag'), /noindex/);
  const data = await report.json(); assert.equal(data.visits, 1); assert.equal(data.downloads, 1); assert.equal(data.countries[0].country, 'DE');
});

test('wrong user and wrong password share response; attempt five locks IP before further hashing', async t => {
  const f = await fixture(t);
  const one = await f.login({ username: 'wrong', password });
  const two = await f.login({ username: 'owner', password: 'wrong' });
  assert.equal(one.status, 401); assert.equal(two.status, 401); assert.deepEqual(await one.json(), await two.json());
  for (let i = 0; i < 3; i++) assert.equal((await f.login({ username: 'owner', password: 'wrong' })).status, 401);
  const blocked = await f.login(); assert.equal(blocked.status, 429); assert.equal(blocked.headers.get('retry-after'), '900');
  // Repeated blocked requests do not extend the fixed window indefinitely.
  f.advance(15 * 60000); assert.equal((await f.login()).status, 202);
});

test('proxy, origin, host, scheme and browser cross-site boundaries reject forged admin requests', async t => {
  const f = await fixture(t), session = await f.auth();
  for (const extra of [
    { 'X-Analytics-Proxy-Key': '' }, { Origin: 'https://evil.example' }, { Origin: f.config.origin },
    { 'X-Analytics-Host': 'evil.example' }, { 'X-Analytics-Proto': 'http' }, { 'Sec-Fetch-Site': 'cross-site' }, { 'X-Admin-Request': '' }
  ]) assert.equal((await f.call('/admin/api/report?days=7', { extra: { Cookie: session.cookie, ...extra } })).status, 403);
  assert.equal((await f.login(undefined, { Origin: '' })).status, 403);
  assert.equal((await f.call('/admin/api/login', { method: 'OPTIONS' })).status, 405);
  const tokenResponse = await f.call('/v1/session', { method: 'POST', body: { challenge: 'test-only' }, extra: { Origin: f.config.origin } });
  const { token } = await tokenResponse.json();
  assert.equal((await f.call('/admin/api/report?days=7', { extra: { Authorization: `Bearer ${token}` } })).status, 401);
});

test('session binding, opaque token tampering and duplicate cookie cannot grant access', async t => {
  const f = await fixture(t), session = await f.auth();
  for (const extra of [
    { 'X-Analytics-IP': '192.0.2.2' }, { 'User-Agent': 'other-browser' },
    { Cookie: '__Host-mukhomor-admin=' + 'a'.repeat(43) }, { Cookie: session.cookie + '; ' + session.cookie }
  ]) assert.equal((await f.call('/admin/api/report?days=7', { extra: { Cookie: session.cookie, ...extra } })).status, 401);
  assert.equal((await f.call('/admin/api/session', { extra: { Cookie: session.cookie } })).status, 200);
});

test('login rotates previous session; logout requires CSRF then revokes cookie replay', async t => {
  const f = await fixture(t), first = await f.auth();
  f.advance(60000);
  const pending = await f.challenge({ Cookie: first.cookie });
  const rotated = await f.verify(pending, pending.code, { Cookie: pending.cookie + '; ' + first.cookie });
  const second = { cookie: rotated.headers.get('set-cookie').split(';')[0], ...await rotated.json() };
  assert.notEqual(first.cookie, second.cookie);
  assert.equal((await f.call('/admin/api/session', { extra: { Cookie: first.cookie } })).status, 401);
  for (const csrf of ['', first.csrf]) assert.equal((await f.call('/admin/api/logout', { method: 'POST', body: {}, extra: { Cookie: second.cookie, 'X-CSRF-Token': csrf } })).status, 403);
  const logout = await f.call('/admin/api/logout', { method: 'POST', body: {}, extra: { Cookie: second.cookie, 'X-CSRF-Token': second.csrf } });
  assert.equal(logout.status, 204); assert.match(logout.headers.get('set-cookie'), /Max-Age=0/);
  assert.equal((await f.call('/admin/api/report?days=7', { extra: { Cookie: second.cookie } })).status, 401);
});

test('idle and absolute expiration enforced even with continuous report requests', async t => {
  const f = await fixture(t), idle = await f.auth();
  f.advance(15 * 60000);
  assert.equal((await f.call('/admin/api/session', { extra: { Cookie: idle.cookie } })).status, 401);
  const active = await f.auth();
  for (let i = 0; i < 4; i++) {
    f.advance(14 * 60000);
    assert.equal((await f.call('/admin/api/report?days=7', { extra: { Cookie: active.cookie } })).status, 200);
  }
  f.advance(4 * 60000);
  assert.equal((await f.call('/admin/api/report?days=7', { extra: { Cookie: active.cookie } })).status, 401);
});

test('static allowlist, methods, bounds and report parameters reject unrelated files and injection', async t => {
  const f = await fixture(t), session = await f.auth();
  assert.equal((await f.call('/admin')).status, 302);
  for (const path of ['/admin/', '/admin/admin.css', '/admin/admin.js', '/admin/assets/Tiny5-Regular.ttf']) assert.equal((await f.call(path)).status, 200);
  for (const path of ['/admin/environment', '/admin/server.mjs', '/admin/assets/../../environment', '/admin/%2e%2e%2fenvironment', '/admin/api/report?days=0', '/admin/api/report?days=36', '/admin/api/report?days=7&days=1', '/admin/api/report?days=1%20OR%201']) {
    const response = await f.call(path, { extra: { Cookie: session.cookie } }); assert.ok(response.status >= 400 && response.status < 500, path);
  }
  assert.equal((await f.login({ username: 'owner', password, extra: 'bad' })).status, 400);
  assert.equal((await f.login({ username: 'owner', password: 'x'.repeat(5000) })).status, 413);
  assert.equal((await f.call('/admin/api/login', { method: 'POST', body: '{' })).status, 400);
  assert.equal((await f.call('/admin/admin.js', { method: 'POST', body: {} })).status, 405);
});

test('IPv6 /64 variants share login budget', async t => {
  const f = await fixture(t);
  for (let i = 1; i <= 5; i++) assert.equal((await f.login({ username: 'owner', password: 'bad' }, { 'X-Analytics-IP': `2001:db8::${i}` })).status, 401);
  assert.equal((await f.login(undefined, { 'X-Analytics-IP': '2001:0db8:0:0::f' })).status, 429);
});

test('scrypt concurrency and shared Telegram delivery budget bound simultaneous login requests', async t => {
  const f = await fixture(t);
  const results = await Promise.all(Array.from({ length: 3 }, (_, i) => f.login(undefined, { 'X-Analytics-IP': `192.0.2.${i + 1}` })));
  assert.deepEqual(results.map(r => r.status).sort(), [202, 429, 429]);
  assert.equal(f.delivered.length, 1);
});

test('persistent global and daily budgets bound distributed attempts across process restarts', () => {
  const dir = mkdtempSync(join(tmpdir(), 'mukhomor-admin-')), file = join(dir, 'test.sqlite');
  let store = openStore(file), time = Date.UTC(2026, 9, 15, 12);
  try {
    for (let i = 0; i < 20; i++) assert.equal(store.reserveAdminAttempt(`ip-${i}`, time), 0);
    store.close(); store = openStore(file);
    assert.equal(store.reserveAdminAttempt('fresh-ip', time), 15 * 60000);
    for (let window = 0; window < 4; window++) {
      time += 15 * 60000;
      for (let i = 0; i < 20; i++) assert.equal(store.reserveAdminAttempt(`ip-${window}-${i}`, time), 0);
    }
    time += 15 * 60000;
    assert.ok(store.reserveAdminAttempt('fresh-ip', time) > 15 * 60000);
    time = Date.UTC(2026, 9, 16);
    assert.equal(store.reserveAdminAttempt('fresh-ip', time), 0);
  } finally {
    store.close();
    for (const suffix of ['', '-wal', '-shm']) if (existsSync(file + suffix)) rmSync(file + suffix);
    rmdirSync(dir);
  }
});

test('only hashed sessions and bindings are stored; credential rotation invalidates a prior session', async t => {
  const f = await fixture(t), session = await f.auth();
  const token = session.cookie.split('=')[1];
  assert.equal(f.store.getAdminSession(token, f.time()), undefined);
  // The service restarts with the same DB but a changed credential version.
  const second = createAnalytics({ ...f.config, admin: { ...f.config.admin, username: 'new-owner' } }, { store: f.store, now: f.time });
  second.server.listen(0, '127.0.0.1'); await once(second.server, 'listening');
  try {
    const result = await fetch(`http://127.0.0.1:${second.server.address().port}/admin/api/session`, { headers: { 'X-Analytics-Proxy-Key': f.config.proxyKey, 'X-Analytics-IP': '192.0.2.1', 'X-Analytics-Proto': 'https', 'X-Analytics-Host': new URL(origin).host, 'X-Admin-Request': '1', Cookie: session.cookie, 'User-Agent': 'test-browser' } });
    assert.equal(result.status, 401);
  } finally { second.server.closeAllConnections(); await new Promise(resolve => second.server.close(resolve)); }
  const frontend = readFileSync(new URL('./admin/admin.js', import.meta.url), 'utf8');
  assert.doesNotMatch(frontend, /localStorage|sessionStorage|innerHTML/);
});

test('password stage exposes neither session nor OTP; incorrect credentials never send Telegram messages', async t => {
  const f = await fixture(t);
  assert.equal((await f.login({ username: 'owner', password: 'bad' })).status, 401);
  assert.equal(f.delivered.length, 0);
  const response = await f.login(), data = await response.json(), cookie = response.headers.get('set-cookie');
  assert.equal(response.status, 202); assert.match(cookie, /^__Host-mukhomor-mfa=/);
  assert.doesNotMatch(cookie, /__Host-mukhomor-admin=/);
  assert.deepEqual(Object.keys(data).sort(), ['challengeRequired', 'expires', 'reference']);
  assert.match(f.delivered[0].code, /^[0-9]{8}$/);
  assert.equal(data.reference, f.delivered[0].reference);
  assert.ok(!JSON.stringify(data).includes(f.delivered[0].code));
  assert.ok(!JSON.stringify(data).includes(telegram.token));
  assert.equal((await f.call('/admin/api/session', { extra: { Cookie: cookie.split(';')[0] } })).status, 401);
  assert.equal((await f.call('/admin/api/verify', { method: 'POST', body: { code: f.delivered[0].code } })).status, 401);
});

test('OTP is single-use, including parallel submissions; a challenge survives a page reload', async t => {
  const f = await fixture(t), pending = await f.challenge();
  const recover = await f.call('/admin/api/challenge', { extra: { Cookie: pending.cookie } });
  assert.equal(recover.status, 200); assert.equal((await recover.json()).reference, pending.reference);
  const responses = await Promise.all([f.verify(pending), f.verify(pending)]);
  assert.deepEqual(responses.map(r => r.status).sort(), [200, 401]);
  assert.equal((await f.verify(pending)).status, 401);
});

test('OTP expires after five minutes and locks after five wrong codes', async t => {
  const f = await fixture(t), pending = await f.challenge();
  const wrong = pending.code === '00000000' ? '11111111' : '00000000';
  for (let i = 0; i < 4; i++) assert.equal((await f.verify(pending, wrong)).status, 400);
  assert.equal((await f.verify(pending, wrong)).status, 401);
  assert.equal((await f.verify(pending)).status, 401);
  f.advance(60000);
  const expired = await f.challenge(); f.advance(5 * 60000);
  assert.equal((await f.verify(expired)).status, 401);
});

test('OTP is bound to browser, IP, credential version and exact challenge', async t => {
  const f = await fixture(t), pending = await f.challenge();
  for (const extra of [{ 'X-Analytics-IP': '192.0.2.9' }, { 'User-Agent': 'other-browser' }]) assert.equal((await f.verify(pending, pending.code, extra)).status, 401);
  for (const extra of [{ Origin: '' }, { Origin: 'https://evil.example' }, { 'X-Admin-Request': '' }, { 'Sec-Fetch-Site': 'cross-site' }]) assert.equal((await f.verify(pending, pending.code, extra)).status, 403);
  f.advance(60000);
  const other = await f.challenge();
  // Test the hash binding directly to avoid the probabilistic coincidence of numeric codes.
  const { createHmac } = await import('node:crypto');
  const mac = (purpose, value) => createHmac('sha256', f.config.secret).update(purpose + '\0' + value).digest('base64url');
  const firstId = mac('admin-challenge', pending.cookie.split('=')[1]);
  const secondId = mac('admin-challenge', other.cookie.split('=')[1]);
  const first = f.store.getAdminChallenge(firstId, f.time()), second = f.store.getAdminChallenge(secondId, f.time());
  assert.equal(f.store.consumeAdminChallenge({ id: secondId, binding: second.binding, version: second.version, codeHash: first.code_hash }, f.time()), 'invalid');
  assert.equal(f.store.consumeAdminChallenge({ id: secondId, binding: second.binding, version: 'changed', codeHash: second.code_hash }, f.time()), 'expired');
  assert.equal((await f.verify(pending)).status, 200);
});

test('new code replaces browser previous challenge; cancellation invalidates a code without a session', async t => {
  const f = await fixture(t), first = await f.challenge();
  const limited = await f.login(); assert.equal(limited.status, 429); assert.equal(f.delivered.length, 1);
  f.advance(60000);
  const second = await f.challenge({ Cookie: first.cookie });
  assert.equal((await f.verify(first)).status, 401);
  const cancel = await f.call('/admin/api/cancel', { method: 'POST', body: {}, extra: { Cookie: second.cookie } });
  assert.equal(cancel.status, 204); assert.match(cancel.headers.get('set-cookie'), /Max-Age=0/);
  assert.equal((await f.verify(second)).status, 401);
});

test('Telegram delivery errors fail closed without exposing code, token or credentials', async t => {
  const f = await fixture(t, {}, async data => { throw new Error(data.code + telegram.token); });
  const response = await f.login();
  assert.equal(response.status, 503); assert.equal(response.headers.get('set-cookie'), null);
  assert.deepEqual(await response.json(), { error: 'code_delivery_failed' });
  assert.equal((await f.call('/admin/api/report?days=7')).status, 401);
  assert.equal((await f.login()).status, 429); assert.equal(f.delivered.length, 1);
});

test('delivery budgets and OTP attempts persist across restart; consumed codes cannot reappear', () => {
  const dir = mkdtempSync(join(tmpdir(), 'mukhomor-otp-')), file = join(dir, 'test.sqlite');
  let store = openStore(file), time = Date.UTC(2026, 9, 15, 12);
  try {
    assert.equal(store.reserveAdminDelivery(time), 0);
    store.addAdminChallenge({ id: 'opaque-hash', binding: 'binding', version: 'version', codeHash: 'code-hmac', reference: 'ABCDEF', expires: time + 300000 }, time);
    assert.equal(store.getAdminChallenge('opaque-hash', time), undefined);
    store.markAdminChallengeReady('opaque-hash', time);
    const attempt = { id: 'opaque-hash', binding: 'binding', version: 'version', codeHash: 'bad-hmac' };
    for (let i = 0; i < 4; i++) assert.equal(store.consumeAdminChallenge(attempt, time), 'invalid');
    store.close(); store = openStore(file);
    assert.equal(store.reserveAdminDelivery(time), 60000);
    assert.equal(store.consumeAdminChallenge(attempt, time), 'expired');
    assert.equal(store.consumeAdminChallenge({ ...attempt, codeHash: 'code-hmac' }, time), 'expired');
    for (let i = 1; i < 5; i++) { time += 60000; assert.equal(store.reserveAdminDelivery(time), 0); }
    time += 60000; assert.ok(store.reserveAdminDelivery(time) > 60000);
    for (let hour = 1; hour <= 3; hour++) {
      time = Date.UTC(2026, 9, 15, 12 + hour);
      for (let i = 0; i < 5; i++) { assert.equal(store.reserveAdminDelivery(time), 0); time += 60000; }
    }
    time = Date.UTC(2026, 9, 15, 16); assert.ok(store.reserveAdminDelivery(time) > 3600000);
    time = Date.UTC(2026, 9, 16); assert.equal(store.reserveAdminDelivery(time), 0);
  } finally {
    store.close(); for (const suffix of ['', '-wal', '-shm']) if (existsSync(file + suffix)) rmSync(file + suffix); rmdirSync(dir);
  }
});
