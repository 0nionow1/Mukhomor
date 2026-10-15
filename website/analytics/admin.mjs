import { randomBytes, randomInt, scrypt } from 'node:crypto';
import { promisify } from 'node:util';
import { existsSync, readFileSync } from 'node:fs';
import { fail, requireThat, equal, readJson } from './http.mjs';
import { sendTelegramCode, validBotToken, validChatId } from './telegram.mjs';
import { CONTENT_LIMIT, loadContentSeed, validateContent, importGithubRelease } from './content.mjs';

const derive = promisify(scrypt);
const COOKIE = '__Host-mukhomor-admin';
const CHALLENGE_COOKIE = '__Host-mukhomor-mfa', CHALLENGE_TTL = 5 * 60000;
const IDLE = 15 * 60000, LIFETIME = 60 * 60000;
// OWASP's 32 MiB scrypt option: N=2^15, r=8, p=3. Bound concurrency separately.
const parameters = { N: 32768, r: 8, p: 3, maxmem: 48 * 1024 * 1024 };
const hashPattern = /^scrypt\$32768\$8\$3\$([a-f0-9]{32})\$([a-f0-9]{64})$/;

export async function hashPassword(password) {
  requireThat(typeof password === 'string' && password.length >= 12 && Buffer.byteLength(password) <= 512, 400, 'Use a password of 12+ characters, up to 512 bytes');
  const salt = randomBytes(16).toString('hex');
  const hash = await derive(password, Buffer.from(salt, 'hex'), 32, parameters);
  return `scrypt$32768$8$3$${salt}$${hash.toString('hex')}`;
}

export function adminConfigFromEnv(env) {
  if (![env.ADMIN_ORIGIN, env.ADMIN_USERNAME, env.ADMIN_PASSWORD_HASH].some(Boolean)) return null;
  const origin = new URL(env.ADMIN_ORIGIN);
  requireThat(origin.protocol === 'https:' && origin.href === origin.origin + '/' && !origin.username && !origin.password, 500, 'Set an HTTPS ADMIN_ORIGIN');
  requireThat(/^[A-Za-z0-9_.-]{3,64}$/.test(env.ADMIN_USERNAME || ''), 500, 'Set ADMIN_USERNAME');
  requireThat(hashPattern.test(env.ADMIN_PASSWORD_HASH || ''), 500, 'Set a valid scrypt ADMIN_PASSWORD_HASH');
  requireThat(validBotToken(env.TELEGRAM_BOT_TOKEN) && validChatId(env.TELEGRAM_CHAT_ID), 500, 'Configure Telegram second factor before enabling admin');
  return { origin: origin.origin, username: env.ADMIN_USERNAME, passwordHash: env.ADMIN_PASSWORD_HASH,
    telegram: { token: env.TELEGRAM_BOT_TOKEN, chatId: env.TELEGRAM_CHAT_ID } };
}

function cookieFrom(request, name = COOKIE) {
  const values = (request.headers.cookie || '').split(';').map(item => item.trim()).filter(item => item.startsWith(name + '='));
  const value = values.length === 1 ? values[0].slice(name.length + 1) : '';
  return /^[A-Za-z0-9_-]{43}$/.test(value) ? value : null;
}
const cookie = (name, value, seconds) => `${name}=${value}; Path=/; HttpOnly; Secure; SameSite=Strict; Max-Age=${seconds}`;

export function createAdmin(config, { store, now, mac, sendCode = sendTelegramCode, importRelease = importGithubRelease }) {
  if (!config.admin) return null;
  const admin = config.admin, origin = new URL(admin.origin);
  const seed=loadContentSeed();
  store.initContent(seed.document,seed.sources);
  let importing=false;
  const match = admin.passwordHash.match(hashPattern);
  requireThat(match, 500, 'Invalid admin password hash');
  requireThat(validBotToken(admin.telegram?.token) && validChatId(admin.telegram?.chatId), 500, 'Telegram second factor is required');
  // Existing password-only sessions and changed Telegram recipients cannot survive this version.
  const credentialVersion = mac('admin-credential', ['telegram-mfa-v1', admin.username, admin.passwordHash, admin.telegram.chatId, admin.telegram.token].join('\0'));
  const assets = existsSync(new URL('./admin/assets/', import.meta.url)) ? new URL('./admin/assets/', import.meta.url) : new URL('../public/assets/', import.meta.url);
  const files = new Map([
    ['/admin/', ['text/html; charset=utf-8', new URL('./admin/index.html', import.meta.url)]],
    ['/admin/admin.css', ['text/css; charset=utf-8', new URL('./admin/admin.css', import.meta.url)]],
    ['/admin/admin.js', ['text/javascript; charset=utf-8', new URL('./admin/admin.js', import.meta.url)]],
    ['/admin/manage.js', ['text/javascript; charset=utf-8', new URL('./admin/manage.js', import.meta.url)]],
    ['/admin/assets/Tiny5-Regular.ttf', ['font/ttf', new URL('Tiny5-Regular.ttf', assets)]],
    ['/admin/assets/mushroom-mini.svg', ['image/svg+xml', new URL('mushroom-mini.svg', assets)]],
    ['/admin/assets/Tiny5-LICENSE.txt', ['text/plain; charset=utf-8', new URL('Tiny5-LICENSE.txt', assets)]]
  ].map(([name, [type, path]]) => [name, { type, data: readFileSync(path) }]));
  let hashing = 0;
  return async (request, response, ip) => {
    const headers = {
      'Cache-Control': 'no-store', 'Content-Type': 'application/json; charset=utf-8',
      'Content-Security-Policy': "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self'; connect-src 'self'; frame-ancestors 'none'; object-src 'none'; base-uri 'none'; form-action 'self'",
      'X-Content-Type-Options': 'nosniff', 'X-Frame-Options': 'DENY',
      'Referrer-Policy': 'no-referrer', 'Cross-Origin-Resource-Policy': 'same-origin',
      'X-Robots-Tag': 'noindex, nofollow, noarchive',
      'Permissions-Policy': 'camera=(), microphone=(), geolocation=()'
    };
    const reply = (status, data) => {
      response.writeHead(status, headers);
      response.end(status === 204 ? undefined : JSON.stringify(data));
    };
    try {
      requireThat(request.headers['x-analytics-proto'] === origin.protocol.slice(0, -1) && request.headers['x-analytics-host'] === origin.host, 403, 'invalid_admin_origin');
      requireThat(!request.headers.origin || request.headers.origin === admin.origin, 403, 'origin_not_allowed');
      requireThat(!request.headers['sec-fetch-site'] || ['same-origin', 'none'].includes(request.headers['sec-fetch-site']), 403, 'cross_site_request');
      if (request.url === '/admin' && request.method === 'GET') {
        response.writeHead(302, { ...headers, Location: '/admin/' }); response.end(); return;
      }
      const file = files.get(request.url);
      if (file) {
        requireThat(['GET', 'HEAD'].includes(request.method), 405, 'method_not_allowed');
        response.writeHead(200, { ...headers, 'Content-Type': file.type });
        response.end(request.method === 'HEAD' ? undefined : file.data); return;
      }
      const url = new URL(request.url, admin.origin);
      requireThat(['/admin/api/login', '/admin/api/challenge', '/admin/api/verify', '/admin/api/cancel', '/admin/api/session', '/admin/api/report', '/admin/api/logout', '/admin/api/site', '/admin/api/site/save', '/admin/api/site/publish', '/admin/api/site/restore', '/admin/api/site/import', '/admin/api/site/export'].includes(url.pathname), 404, 'not_found');
      requireThat(request.headers['x-admin-request'] === '1', 403, 'admin_header_required');
      const time = now();
      const binding = mac('admin-binding', ip.full + '\0' + (request.headers['user-agent'] || '').slice(0, 512));
      const rawCookie = cookieFrom(request);
      const sessionId = rawCookie ? mac('admin-session', rawCookie) : null;
      const rawChallenge = cookieFrom(request, CHALLENGE_COOKIE);
      const challengeId = rawChallenge ? mac('admin-challenge', rawChallenge) : null;
      if (url.pathname === '/admin/api/login') {
        requireThat(request.method === 'POST' && !url.search, 405, 'method_not_allowed');
        requireThat(request.headers.origin === admin.origin, 403, 'origin_not_allowed');
        const body = await readJson(request);
        requireThat(Object.keys(body).length === 2 && typeof body.username === 'string' && body.username.length <= 64 && typeof body.password === 'string' && Buffer.byteLength(body.password) <= 512);
        requireThat(hashing < 2, 429, 'login_limited');
        // Persistent IP, global-window and global-day budgets bound distributed guessing.
        const wait = store.reserveAdminAttempt(mac('admin-ip', ip.network), time);
        if (wait) { headers['Retry-After'] = String(Math.ceil(wait / 1000)); throw fail(429, 'login_limited'); }
        hashing++;
        let candidate;
        try { candidate = await derive(body.password, Buffer.from(match[1], 'hex'), 32, parameters); }
        finally { hashing--; body.password = ''; }
        // Always derive the password even for an unknown username.
        const passwordOK = equal(candidate.toString('hex'), match[2]);
        const userOK = equal(mac('admin-user', body.username), mac('admin-user', admin.username));
        requireThat(passwordOK && userOK, 401, 'invalid_credentials');
        const waitForDelivery = store.reserveAdminDelivery(now());
        if (waitForDelivery) { headers['Retry-After'] = String(Math.ceil(waitForDelivery / 1000)); throw fail(429, 'delivery_limited'); }
        const token = randomBytes(32).toString('base64url'), id = mac('admin-challenge', token);
        const reference = randomBytes(3).toString('hex').toUpperCase(), expires = now() + CHALLENGE_TTL;
        let code = String(randomInt(100000000)).padStart(8, '0');
        store.addAdminChallenge({ id, binding, version: credentialVersion, reference, expires, codeHash: mac('admin-otp', id + '\0' + code) }, now());
        try {
          await sendCode({ telegram: admin.telegram, code, reference, origin: admin.origin });
          requireThat(store.markAdminChallengeReady(id, now()), 503, 'delivery_failed');
        } catch {
          store.deleteAdminChallenge(id);
          throw fail(503, 'code_delivery_failed');
        } finally { code = ''; }
        if (challengeId) store.deleteAdminChallenge(challengeId);
        headers['Set-Cookie'] = cookie(CHALLENGE_COOKIE, token, CHALLENGE_TTL / 1000);
        return reply(202, { challengeRequired: true, reference, expires });
      }
      if (['/admin/api/challenge', '/admin/api/verify', '/admin/api/cancel'].includes(url.pathname)) {
        requireThat(!url.search);
        requireThat(request.method === (url.pathname.endsWith('/challenge') ? 'GET' : 'POST'), 405, 'method_not_allowed');
        if (request.method === 'POST') requireThat(request.headers.origin === admin.origin, 403, 'origin_not_allowed');
        const challenge = challengeId ? store.getAdminChallenge(challengeId, now()) : null;
        if (url.pathname.endsWith('/cancel')) {
          const body = await readJson(request); requireThat(Object.keys(body).length === 0);
          if (challenge && equal(challenge.binding, binding) && equal(challenge.version, credentialVersion)) store.deleteAdminChallenge(challengeId);
          headers['Set-Cookie'] = cookie(CHALLENGE_COOKIE, '', 0);
          return reply(204);
        }
        requireThat(challenge && equal(challenge.binding, binding) && equal(challenge.version, credentialVersion), 401, 'challenge_expired');
        if (request.method === 'GET') return reply(200, { challengeRequired: true, reference: challenge.reference, expires: challenge.expires });
        const body = await readJson(request);
        requireThat(Object.keys(body).length === 1 && typeof body.code === 'string' && /^[0-9]{8}$/.test(body.code));
        const result = store.consumeAdminChallenge({ id: challengeId, binding, version: credentialVersion, codeHash: mac('admin-otp', challengeId + '\0' + body.code) }, now());
        body.code = '';
        requireThat(result !== 'expired', 401, 'challenge_expired');
        requireThat(result === 'accepted', 400, 'invalid_code');
        if (sessionId) store.deleteAdminSession(sessionId);
        const token = randomBytes(32).toString('base64url');
        const id = mac('admin-session', token), created = now(), expires = created + LIFETIME;
        store.addAdminSession({ id, binding, version: credentialVersion, expires, idle: created + IDLE }, created);
        headers['Set-Cookie'] = [cookie(COOKIE, token, LIFETIME / 1000), cookie(CHALLENGE_COOKIE, '', 0)];
        return reply(200, { username: admin.username, csrf: mac('admin-csrf', id), expires, idle: created + IDLE });
      }
      const session = sessionId ? store.getAdminSession(sessionId, time) : null;
      requireThat(session && equal(session.binding, binding) && equal(session.version, credentialVersion), 401, 'authentication_required');
      if (url.pathname.startsWith('/admin/api/site')) {
        requireThat(!url.search,400,'invalid_request');
        store.touchAdminSession(sessionId,Math.min(time+IDLE,session.expires));
        if (['/admin/api/site','/admin/api/site/export'].includes(url.pathname)) {
          requireThat(request.method==='GET',405,'method_not_allowed');
          if (url.pathname.endsWith('/export')) {
            const published=store.getPublishedContent();requireThat(published,404,'site_not_published');
            headers['Content-Disposition']='attachment; filename="mukhomor-content.json"';
            return reply(200,published.document);
          }
          return reply(200,{...store.getContent(),fields:seed.fields});
        }
        requireThat(request.method==='POST',405,'method_not_allowed');
        requireThat(request.headers.origin===admin.origin && equal(request.headers['x-csrf-token'],mac('admin-csrf',sessionId)),403,'csrf_failed');
        const wait=store.reserveContentWrite(time,url.pathname.endsWith('/import'));
        if(wait){headers['Retry-After']=String(Math.ceil(wait/1000));throw fail(429,'content_rate_limited');}
        const body=await readJson(request,url.pathname.endsWith('/save')?CONTENT_LIMIT+1024:4096);
        if(url.pathname.endsWith('/import')) {
          requireThat(Object.keys(body).length===1 && typeof body.version==='string');
          requireThat(!importing,429,'github_import_busy');importing=true;
          let source;try{source=await importRelease(body.version);}finally{importing=false;}
          store.addContentSource(source);
          return reply(200,{...source,title:`Мухомор ${source.version}`,changes:['Описание обновления'],status:'testing',visible:true});
        }
        requireThat(Number.isSafeInteger(body.revision) && body.revision>0,400,'invalid_revision');
        if(url.pathname.endsWith('/save')) {
          requireThat(Object.keys(body).length===2);
          const document=validateContent(body.document,seed.fields,store.contentSources());
          store.saveContent(document,body.revision,time);
        } else if(url.pathname.endsWith('/publish')) {
          requireThat(Object.keys(body).length===1);
          validateContent(store.getContent().document,seed.fields,store.contentSources());
          store.publishContent(body.revision,time);
        } else {
          requireThat(Object.keys(body).length===2 && Number.isSafeInteger(body.fromRevision));
          store.restoreContent(body.revision,body.fromRevision,time);
        }
        return reply(200,{...store.getContent(),fields:seed.fields});
      }
      if (url.pathname === '/admin/api/logout') {
        requireThat(request.method === 'POST' && !url.search, 405, 'method_not_allowed');
        requireThat(request.headers.origin === admin.origin && equal(request.headers['x-csrf-token'], mac('admin-csrf', sessionId)), 403, 'csrf_failed');
        const body = await readJson(request); requireThat(Object.keys(body).length === 0);
        store.deleteAdminSession(sessionId);
        if (challengeId) store.deleteAdminChallenge(challengeId);
        headers['Set-Cookie'] = [cookie(COOKIE, '', 0), cookie(CHALLENGE_COOKIE, '', 0)];
        return reply(204);
      }
      requireThat(request.method === 'GET', 405, 'method_not_allowed');
      if (url.pathname === '/admin/api/session') {
        requireThat(!url.search);
        return reply(200, { username: admin.username, csrf: mac('admin-csrf', sessionId), expires: session.expires, idle: session.idle });
      }
      requireThat(/^\?days=(?:[1-9]|[12][0-9]|3[0-5])$/.test(url.search));
      store.touchAdminSession(sessionId, Math.min(time + IDLE, session.expires));
      return reply(200, store.report(Number(url.searchParams.get('days')), time));
    } catch (error) {
      if (error.status === 429 && !headers['Retry-After']) headers['Retry-After'] = '60';
      reply(error.status || 503, { error: error.status ? error.message : 'temporarily_unavailable' });
      request.resume();
    }
  };
}
