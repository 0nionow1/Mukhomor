import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, writeFileSync, readFileSync, readdirSync, rmSync, rmdirSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { sendTelegramCode, telegramRequest, validChatId } from './telegram.mjs';
import { beginPairing, completePairing, pairedChat, readPrivateEnvironment } from './telegram-setup.mjs';

const token = '123456:' + 'b'.repeat(35), chatId = '123456789';
const now = Date.UTC(2026, 9, 15, 12);
const fixtureMessage = (nonce, id = Number(chatId)) => ({ update_id: 42, message: { date: now / 1000, text: '/start mk_' + nonce, chat: { id, type: 'private' }, from: { id, is_bot: false } } });

test('Telegram code goes only to configured private chat over fixed HTTPS API; responses never returned to frontend', async () => {
  let requests = 0;
  const result = await sendTelegramCode({ telegram: { token, chatId }, code: '01234567', reference: 'ABCDEF', origin: 'https://stats.example.test' }, async (url, options) => {
    requests++;
    assert.equal(url, `https://api.telegram.org/bot${token}/sendMessage`);
    assert.equal(options.method, 'POST'); assert.equal(options.redirect, 'error'); assert.ok(options.signal);
    const body = JSON.parse(options.body);
    assert.equal(body.chat_id, chatId); assert.equal(body.protect_content, true);
    assert.equal(body.allow_paid_broadcast, undefined); assert.equal(body.parse_mode, undefined);
    assert.match(body.text, /01234567/); assert.match(body.text, /ABCDEF/); assert.match(body.text, /5 минут/);
    return Response.json({ ok: true, result: { message_id: 9, chat: { id: Number(chatId), type: 'private' } } });
  });
  assert.equal(requests, 1); assert.equal(result, undefined);
  for (const id of ['-12345', '@channel', '1e5', '9007199254740992', '0']) assert.equal(validChatId(id), false);
});

test('Telegram timeouts, rate limit, redirects, malformed/oversized responses and wrong recipient fail closed', async () => {
  const data = { telegram: { token, chatId }, code: '01234567', reference: 'ABCDEF', origin: 'https://stats.example.test' };
  for (const fake of [
    async () => { throw new Error('Private error ' + token); },
    async () => new Response('secret ' + token, { status: 429 }),
    async () => new Response('', { status: 302, headers: { Location: 'https://evil.example' } }),
    async () => Response.json({ ok: false, description: token }),
    async () => new Response('not-json'),
    async () => new Response('x'.repeat(65537)),
    async () => Response.json({ ok: true, result: { message_id: 1, chat: { id: Number(chatId), type: 'group' } } }),
    async () => Response.json({ ok: true, result: { message_id: 1, chat: { id: 987, type: 'private' } } })
  ]) await assert.rejects(sendTelegramCode(data, fake), error => error.status === 503 && !error.message.includes(token) && !error.message.includes(data.code));
  await assert.rejects(telegramRequest(token, 'deleteWebhook'), /method_not_allowed/);
  await assert.rejects(telegramRequest('invalid/token', 'getMe'), /not_configured/);
});

test('pairing requires a fresh unique nonce from a human private chat; never first sender or group', () => {
  const state = { nonce: 'a'.repeat(48), started: now, expires: now + 1800000 };
  const valid = fixtureMessage(state.nonce);
  const unrelated = fixtureMessage('b'.repeat(48), 987);
  assert.equal(pairedChat([unrelated, valid], state, now), chatId);
  assert.throws(() => pairedChat([unrelated], state, now));
  assert.throws(() => pairedChat([valid, fixtureMessage(state.nonce, 987)], state, now), /multiple chats/);
  for (const change of [
    m => { m.chat.type = 'group'; }, m => { m.from.is_bot = true; }, m => { m.from.id = 999; },
    m => { m.date -= 3600; }, m => { m.text = '/start'; }
  ]) { const update = structuredClone(valid); change(update.message); assert.throws(() => pairedChat([update], state, now)); }
  assert.throws(() => pairedChat([valid], state, state.expires), /expired/);
});

test('one-shot pairing saves private chat atomically, preserves existing settings and cannot be replayed', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'mukhomor-telegram-')), env = join(dir, 'environment');
  writeFileSync(env, `# private test\nTURNSTILE_SECRET=fixture-only\nTELEGRAM_BOT_SECRET=${token}\n`);
  try {
    const calls = [];
    const begin = await beginPairing(env, { now, request: async (_token, method) => {
      calls.push(method); return method === 'getMe' ? { is_bot: true, username: 'test_fixture_bot' } : { url: '' };
    } });
    assert.deepEqual(calls, ['getMe', 'getWebhookInfo']);
    const nonce = new URL(begin.link).searchParams.get('start').slice(3);
    const result = await completePairing(env, { now, request: async (_token, method, body) => {
      assert.equal(method, 'getUpdates'); assert.deepEqual(body, { limit: 100, timeout: 0 }); return [fixtureMessage(nonce)];
    } });
    assert.deepEqual(result, { paired: true, chatType: 'private' });
    const parsed = readPrivateEnvironment(env);
    assert.equal(parsed.values.TELEGRAM_CHAT_ID, chatId); assert.equal(parsed.values.TURNSTILE_SECRET, 'fixture-only');
    assert.equal(parsed.values.TELEGRAM_BOT_TOKEN, token); assert.equal(parsed.values.TELEGRAM_BOT_SECRET, undefined);
    assert.ok(!readFileSync(env, 'utf8').includes(nonce));
    await assert.rejects(completePairing(env, { now }), /ENOENT/);
    await assert.rejects(beginPairing(env, { now, request: async (_token, method) => method === 'getMe' ? { is_bot: true, username: 'test_fixture_bot' } : { url: 'https://existing.example' } }), /webhook/);
  } finally { for (const file of readdirSync(dir)) rmSync(join(dir, file)); rmdirSync(dir); }
});
