// One-shot local operator utility. No webhook, polling daemon or HTTP setup route.
import { randomBytes, createHash } from 'node:crypto';
import { readFileSync, writeFileSync, renameSync, chmodSync, lstatSync, existsSync, unlinkSync } from 'node:fs';
import { isAbsolute, dirname, join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { telegramRequest, validBotToken, validChatId } from './telegram.mjs';

function check(condition, message) { if (!condition) throw new Error(message); }
export function readPrivateEnvironment(path) {
  check(isAbsolute(path) && lstatSync(path).isFile() && !lstatSync(path).isSymbolicLink(), 'Use an absolute path to a private regular environment file.');
  const text = readFileSync(path, 'utf8'), values = {};
  for (const line of text.split(/\r?\n/)) {
    if (!line.trim() || line.trim().startsWith('#')) continue;
    const match = line.match(/^\s*([A-Z][A-Z0-9_]*)\s*=\s*(.*?)\s*$/);
    check(match && !Object.hasOwn(values, match[1]), 'Malformed or duplicated private environment entry.');
    const value = match[2]; values[match[1]] = /^(["']).*\1$/.test(value) ? value.slice(1, -1) : value;
  }
  const token = values.TELEGRAM_BOT_TOKEN || values.TELEGRAM_BOT_SECRET;
  check(validBotToken(token), 'Set TELEGRAM_BOT_TOKEN in the private environment file.');
  check(!values.TELEGRAM_BOT_TOKEN || !values.TELEGRAM_BOT_SECRET || values.TELEGRAM_BOT_TOKEN === values.TELEGRAM_BOT_SECRET, 'Conflicting Telegram token entries.');
  return { text, values, token };
}
function atomicPrivateWrite(path, text) {
  if (existsSync(path)) check(lstatSync(path).isFile() && !lstatSync(path).isSymbolicLink(), 'Refusing to replace a non-regular private file.');
  const temp = path + '.' + randomBytes(8).toString('hex') + '.tmp';
  try { writeFileSync(temp, text, { mode: 0o600, flag: 'wx' }); renameSync(temp, path); chmodSync(path, 0o600); }
  finally { if (existsSync(temp)) unlinkSync(temp); }
}
const fingerprint = token => createHash('sha256').update(token).digest('hex');
const statePath = path => join(dirname(path), '.telegram-pairing.json');

export async function beginPairing(path, { now = Date.now(), request = telegramRequest } = {}) {
  const { token } = readPrivateEnvironment(path);
  const bot = await request(token, 'getMe');
  const webhook = await request(token, 'getWebhookInfo');
  check(bot.is_bot === true && /^[A-Za-z0-9_]{5,32}$/.test(bot.username || ''), 'Telegram did not return a valid bot.');
  check(!webhook.url, 'This bot has a webhook. Use a dedicated bot; existing webhooks are not changed automatically.');
  const nonce = randomBytes(24).toString('hex');
  atomicPrivateWrite(statePath(path), JSON.stringify({ nonce, tokenHash: fingerprint(token), started: now, expires: now + 30 * 60000 }) + '\n');
  return { link: `https://t.me/${bot.username}?start=mk_${nonce}`, expiresInMinutes: 30 };
}

export function pairedChat(updates, state, now) {
  check(Array.isArray(updates) && /^[a-f0-9]{48}$/.test(state.nonce) && Number.isSafeInteger(state.started) && Number.isSafeInteger(state.expires) && state.expires > now && state.started <= now, 'Pairing expired or invalid. Start a new pairing.');
  const matches = updates.filter(update => {
    const m = update.message;
    return m && m.chat?.type === 'private' && m.from?.is_bot === false && m.from.id === m.chat.id &&
      validChatId(String(m.chat.id)) && m.text === '/start mk_' + state.nonce &&
      Number.isInteger(m.date) && m.date * 1000 >= state.started - 60000 && m.date * 1000 <= now + 60000;
  });
  const ids = [...new Set(matches.map(update => String(update.message.chat.id)))];
  check(ids.length === 1, ids.length ? 'Pairing was used by multiple chats. Start again and keep the link private.' : 'Pairing message not found. Open the link, press Start, then run complete again.');
  return ids[0];
}

export async function completePairing(path, { now = Date.now(), request = telegramRequest } = {}) {
  const { text, token } = readPrivateEnvironment(path);
  const state = JSON.parse(readFileSync(statePath(path), 'utf8'));
  check(state.tokenHash === fingerprint(token) && state.expires > now, 'Bot changed or pairing expired. Start a new pairing.');
  const updates = await request(token, 'getUpdates', { limit: 100, timeout: 0 });
  const chatId = pairedChat(updates, state, now);
  // Do not select the first sender, acknowledge unrelated updates, or print messages.
  const clean = text.split(/\r?\n/).filter(line => !/^\s*TELEGRAM_(BOT_TOKEN|BOT_SECRET|CHAT_ID)\s*=/.test(line)).join('\n').trimEnd();
  atomicPrivateWrite(path, clean + `\nTELEGRAM_BOT_TOKEN=${token}\nTELEGRAM_CHAT_ID=${chatId}\n`);
  unlinkSync(statePath(path));
  return { paired: true, chatType: 'private' };
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    const [command, flag, path, ...extra] = process.argv.slice(2);
    check(['begin', 'complete'].includes(command) && flag === '--env' && path && !extra.length, 'Usage: telegram-setup.mjs begin|complete --env /absolute/private/environment');
    console.log(JSON.stringify(await (command === 'begin' ? beginPairing(path) : completePairing(path))));
  } catch (error) {
    // Never include error causes, response bodies, tokens, codes or unrelated updates.
    console.error(error.status ? 'Telegram API unavailable. Check the private token and network.' : error.message.startsWith('ENOENT:') ? 'Private environment or pairing file is missing.' : error instanceof SyntaxError ? 'Invalid private pairing file.' : error.message);
    process.exitCode = 1;
  }
}
