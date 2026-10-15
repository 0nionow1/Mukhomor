import { createServer } from 'node:http';
import { createHmac, randomUUID } from 'node:crypto';
import { isIP } from 'node:net';
import { mkdirSync, chmodSync } from 'node:fs';
import { dirname, isAbsolute } from 'node:path';
import { pathToFileURL } from 'node:url';
import { openStore } from './store.mjs';
import { fail, requireThat, equal, readJson } from './http.mjs';
import { createAdmin, adminConfigFromEnv } from './admin.mjs';
import { publicContent } from './content.mjs';

const SESSION_MS = 30 * 60000, VISITOR_MS = 30 * 86400000;

export function configFromEnv(env = process.env) {
  const config = {
    origin: 'https://0nionow1.github.io', hostname: '0nionow1.github.io',
    secret: env.ANALYTICS_SECRET, proxyKey: env.ANALYTICS_PROXY_KEY,
    turnstileSecret: env.TURNSTILE_SECRET, dbPath: env.ANALYTICS_DB,
    port: Number(env.PORT || 8787), dailySessions: Number(env.DAILY_SESSIONS || 4000),
    dailyEvents: Number(env.DAILY_EVENTS || 20000), admin: adminConfigFromEnv(env)
  };
  for (const key of ['secret', 'proxyKey']) {
    requireThat(typeof config[key] === 'string' && /^[a-f0-9]{64,128}$/i.test(config[key]), 500, `Set a random hexadecimal ${key}`);
  }
  requireThat(config.secret !== config.proxyKey, 500, 'Use different secrets');
  requireThat(typeof config.turnstileSecret === 'string' && config.turnstileSecret.length >= 20 && !config.turnstileSecret.includes('REPLACE'), 500, 'Set Turnstile secret');
  requireThat(config.dbPath && isAbsolute(config.dbPath), 500, 'Set an absolute private database path');
  for (const key of ['port', 'dailySessions', 'dailyEvents']) requireThat(Number.isInteger(config[key]) && config[key] > 0 && config[key] <= (key === 'port' ? 65535 : 100000), 500, `Invalid ${key}`);
  return config;
}

// Canonical IPs prevent IPv6 spelling variants from bypassing quotas. Rate IPv6 by /64.
export function normalizeIP(value) {
  requireThat(typeof value === 'string' && value.length < 64 && !value.includes('%') && isIP(value), 403, 'invalid_peer');
  if (isIP(value) === 4) return { full: value, network: value };
  const full = new URL(`http://[${value}]/`).hostname.slice(1, -1);
  const halves = full.split('::');
  const left = halves[0] ? halves[0].split(':') : [];
  const right = halves[1] ? halves[1].split(':') : [];
  const parts = halves.length === 2 ? [...left, ...Array(8 - left.length - right.length).fill('0'), ...right] : left;
  const words = parts.map(word => parseInt(word, 16));
  if (words.slice(0, 5).every(word => word === 0) && words[5] === 65535) {
    const ipv4 = [words[6] >> 8, words[6] & 255, words[7] >> 8, words[7] & 255].join('.');
    return { full: ipv4, network: ipv4 };
  }
  return { full, network: words.slice(0, 4).map(word => word.toString(16)).join(':') + '::/64' };
}

class Limits {
  entries = new Map();
  take(key, maximum, window, now) {
    let item = this.entries.get(key);
    if (item && item.until <= now) { this.entries.delete(key); item = null; }
    if (!item) {
      if (this.entries.size >= 20000) {
        for (const [name, value] of this.entries) if (value.until <= now) this.entries.delete(name);
        if (this.entries.size >= 20000) return false;
      }
      item = { until: now + window, count: 0 }; this.entries.set(key, item);
    }
    return ++item.count <= maximum;
  }
}

export async function verifyTurnstile({ token, ip, secret }, fetcher = fetch) {
  const response = await fetcher('https://challenges.cloudflare.com/turnstile/v0/siteverify', {
    method: 'POST', body: new URLSearchParams({ secret, response: token, remoteip: ip }),
    signal: AbortSignal.timeout(5000), redirect: 'error'
  });
  if (!response.ok) throw fail(503, 'verification_unavailable');
  return response.json();
}

export function createAnalytics(config, { store = openStore(config.dbPath), now = Date.now, verify = verifyTurnstile, sendAdminCode, importRelease } = {}) {
  const limits = new Limits();
  let verifying = 0, lastCleanup = 0;
  const mac = (purpose, text) => createHmac('sha256', config.secret).update(purpose + '\0' + text).digest('base64url');
  const adminHandler = createAdmin(config, { store, now, mac, sendCode: sendAdminCode, importRelease });
  function sign(payload) {
    const body = Buffer.from(JSON.stringify({ ...payload, aud: config.origin })).toString('base64url');
    return `${body}.${mac('token', body)}`;
  }
  function decode(token, purpose, time) {
    requireThat(typeof token === 'string' && token.length <= 1800, 401, 'invalid_token');
    const parts = token.split('.');
    requireThat(parts.length === 2 && equal(mac('token', parts[0]), parts[1]), 401, 'invalid_token');
    let data;
    try { data = JSON.parse(Buffer.from(parts[0], 'base64url').toString('utf8')); } catch { throw fail(401, 'invalid_token'); }
    requireThat(data && data.type === purpose && data.aud === config.origin && Number.isSafeInteger(data.exp) && data.exp > time, 401, 'expired_token');
    return data;
  }
  const server = createServer({ maxHeaderSize: 8192, requestTimeout: 10000, headersTimeout: 10000, keepAliveTimeout: 3000 }, async (request, response) => {
    const headers = { 'Cache-Control': 'no-store', 'Content-Type': 'application/json', 'X-Content-Type-Options': 'nosniff', 'Vary': 'Origin' };
    const reply = (status, data) => {
      response.writeHead(status, { ...headers, ...(status === 429 || status === 503 ? { 'Retry-After': '60' } : {}) });
      response.end(status === 204 ? undefined : JSON.stringify(data));
    };
    try {
      const time = now();
      requireThat(limits.take('global', 3000, 60000, time), 429, 'rate_limited');
      requireThat(['127.0.0.1', '::1', '::ffff:127.0.0.1'].includes(request.socket.remoteAddress) && equal(request.headers['x-analytics-proxy-key'], config.proxyKey), 403, 'untrusted_proxy');
      const ip = normalizeIP(request.headers['x-analytics-ip']);
      requireThat(limits.take(`ip:${mac('rate', ip.network)}`, 120, 60000, time), 429, 'rate_limited');
      if (request.url === '/admin' || request.url.startsWith('/admin/')) {
        requireThat(adminHandler, 404, 'not_found');
        return await adminHandler(request, response, ip);
      }
      if (request.url === '/v1/site') {
        requireThat(!request.headers.origin || request.headers.origin===config.origin,403,'origin_not_allowed');
        requireThat(['GET','HEAD'].includes(request.method),405,'method_not_allowed');
        const published=store.getPublishedContent();
        requireThat(published,503,'site_not_published');
        headers['Access-Control-Allow-Origin']=config.origin;
        headers['Cache-Control']='public, max-age=30, must-revalidate';
        headers.ETag=`"site-${published.revision}"`;
        if(request.headers['if-none-match']===headers.ETag){response.writeHead(304,headers);return response.end();}
        response.writeHead(200,headers);
        return response.end(request.method==='HEAD' ? undefined : JSON.stringify({...published,document:publicContent(published.document)}));
      }
      requireThat(request.headers.origin === config.origin, 403, 'origin_not_allowed');
      headers['Access-Control-Allow-Origin'] = config.origin;
      requireThat(['/v1/session', '/v1/event'].includes(request.url), 404, 'not_found');
      if (request.method === 'OPTIONS') {
        requireThat(request.headers['access-control-request-method'] === 'POST', 405, 'method_not_allowed');
        const names = (request.headers['access-control-request-headers'] || '').toLowerCase().split(',').map(name => name.trim()).filter(Boolean);
        requireThat(names.every(name => ['content-type', 'authorization'].includes(name)), 403, 'headers_not_allowed');
        headers['Access-Control-Allow-Methods'] = 'POST';
        headers['Access-Control-Allow-Headers'] = 'Content-Type, Authorization';
        headers['Access-Control-Max-Age'] = '600';
        return reply(204);
      }
      requireThat(request.method === 'POST', 405, 'method_not_allowed');
      const binding = mac('binding', ip.full + '\0' + (request.headers['user-agent'] || '').slice(0, 512));
      const body = await readJson(request);
      if (time - lastCleanup > 3600000) { store.cleanup(time); lastCleanup = time; }
      if (request.url === '/v1/session') {
        requireThat(Object.keys(body).every(key => ['challenge', 'visitorToken'].includes(key)) && typeof body.challenge === 'string' && body.challenge.length > 0 && body.challenge.length <= 2048);
        requireThat(body.visitorToken === undefined || (typeof body.visitorToken === 'string' && body.visitorToken.length <= 1800));
        requireThat(limits.take(`issue:${mac('rate', ip.network)}`, 4, 600000, time), 429, 'rate_limited');
        requireThat(verifying < 8, 503, 'verification_busy');
        verifying++;
        let verification;
        try { verification = await verify({ token: body.challenge, ip: ip.full, secret: config.turnstileSecret }); }
        catch { throw fail(503, 'verification_unavailable'); }
        finally { verifying--; }
        const checkedAt = now();
        requireThat(verification.success === true && verification.hostname === config.hostname && verification.action === 'mukhomor-analytics', 403, 'verification_failed');
        let visitor;
        if (body.visitorToken) { try { visitor = decode(body.visitorToken, 'visitor', checkedAt); } catch { /* A fresh challenge permits a fresh visitor. */ } }
        if (!visitor) visitor = { type: 'visitor', id: randomUUID(), exp: checkedAt + VISITOR_MS };
        const id = randomUUID(), expires = checkedAt + SESSION_MS;
        const suppliedCountry = request.headers['x-analytics-country'];
        const country = /^[A-Z]{2}$/.test(suppliedCountry || '') && suppliedCountry !== 'XX' ? suppliedCountry : 'ZZ';
        requireThat(store.addSession({ id, visitor: mac('visitor', visitor.id), country, expires }, checkedAt, config.dailySessions), 503, 'daily_budget');
        return reply(200, { token: sign({ type: 'session', id, binding, exp: expires }), expires, visitorToken: sign(visitor), visitorExpires: visitor.exp });
      }
      requireThat(Object.keys(body).length === 3 && ['id', 'kind', 'page'].every(key => Object.hasOwn(body, key)));
      requireThat(typeof body.id === 'string' && /^[a-f0-9]{8}-[a-f0-9]{4}-4[a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}$/i.test(body.id));
      requireThat(['page_view', 'github', 'download'].includes(body.kind) && ['home', 'docs', 'changelog', 'faq', 'privacy', 'policy', 'releases'].includes(body.page));
      const authorization = request.headers.authorization || '';
      requireThat(authorization.startsWith('Bearer '), 401, 'token_required');
      const session = decode(authorization.slice(7), 'session', time);
      requireThat(equal(session.binding, binding), 401, 'binding_changed');
      const status = store.record(session.id, body, time, config.dailyEvents);
      reply(status, status === 204 ? undefined : { error: status === 401 ? 'expired_token' : 'event_rejected' });
    } catch (error) {
      // Never log request bodies, addresses, challenge responses, or credentials.
      reply(error.status || 503, { error: error.status ? error.message : 'temporarily_unavailable' });
      request.resume();
    }
  });
  server.maxConnections = 128;
  server.maxRequestsPerSocket = 50;
  server.setTimeout(10000, socket => socket.destroy());
  server.on('clientError', (_error, socket) => socket.destroy());
  return { server, store };
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  process.umask(0o077);
  const config = configFromEnv();
  mkdirSync(dirname(config.dbPath), { recursive: true, mode: 0o700 });
  const { server, store } = createAnalytics(config);
  chmodSync(config.dbPath, 0o600);
  store.cleanup(Date.now());
  const cleanup = setInterval(() => {
    try { store.cleanup(Date.now()); } catch { console.error('Analytics maintenance failed. Check private storage.'); }
  }, 3600000).unref();
  server.listen(config.port, '127.0.0.1', () => console.log('Analytics listening on loopback.'));
  for (const signal of ['SIGINT', 'SIGTERM']) process.on(signal, () => server.close(() => { clearInterval(cleanup); store.close(); process.exit(0); }));
}
