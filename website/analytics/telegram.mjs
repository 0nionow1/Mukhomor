import { fail, requireThat } from './http.mjs';

export const validBotToken = value => typeof value === 'string' && /^[0-9]{5,20}:[A-Za-z0-9_-]{30,100}$/.test(value);
export const validChatId = value => typeof value === 'string' && /^[1-9][0-9]{0,15}$/.test(value) && Number.isSafeInteger(Number(value));

// Fixed API origin and method allowlist: neither comes from an HTTP request.
export async function telegramRequest(token, method, body = {}, fetcher = fetch) {
  requireThat(validBotToken(token), 503, 'telegram_not_configured');
  requireThat(['sendMessage', 'getMe', 'getUpdates', 'getWebhookInfo'].includes(method), 503, 'telegram_method_not_allowed');
  try {
    const response = await fetcher(`https://api.telegram.org/bot${token}/${method}`, {
      method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body),
      signal: AbortSignal.timeout(5000), redirect: 'error'
    });
    if (!response.ok) { await response.body?.cancel(); throw new Error(); }
    const chunks = []; let size = 0;
    for await (const chunk of response.body) {
      size += chunk.length;
      if (size > (method === 'getUpdates' ? 1048576 : 65536)) throw new Error();
      chunks.push(chunk);
    }
    const data = JSON.parse(Buffer.concat(chunks).toString('utf8'));
    if (data.ok !== true || !data.result) throw new Error();
    return data.result;
  } catch {
    // Fetch errors and Telegram responses can contain the bot token or OTP.
    throw fail(503, 'telegram_unavailable');
  }
}

export async function sendTelegramCode({ telegram, code, reference, origin }, fetcher = fetch) {
  requireThat(validChatId(telegram.chatId) && /^[0-9]{8}$/.test(code) && /^[A-F0-9]{6}$/.test(reference), 503, 'telegram_not_configured');
  const message = await telegramRequest(telegram.token, 'sendMessage', {
    chat_id: telegram.chatId,
    text: `Мухомор · вход в панель\n\nКод: ${code}\nЗапрос: ${reference}\nАдрес: ${origin}/admin/\n\nКод действует 5 минут и подходит только для этого входа. Если вход запросили не вы, не вводите код и смените пароль панели.`,
    protect_content: true, link_preview_options: { is_disabled: true }
  }, fetcher);
  requireThat(message.chat?.type === 'private' && String(message.chat.id) === telegram.chatId && Number.isSafeInteger(message.message_id), 503, 'telegram_delivery_failed');
}
