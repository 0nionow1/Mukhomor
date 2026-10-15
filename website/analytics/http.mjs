import { timingSafeEqual } from 'node:crypto';

export const fail = (status, code) => Object.assign(new Error(code), { status });
export const requireThat = (condition, status = 400, code = 'invalid_request') => { if (!condition) throw fail(status, code); };
export const equal = (a, b) => typeof a === 'string' && typeof b === 'string' && Buffer.byteLength(a) === Buffer.byteLength(b) && timingSafeEqual(Buffer.from(a), Buffer.from(b));

export async function readJson(request, limit = 4096) {
  requireThat(/^application\/json(?:\s*;\s*charset=utf-8)?$/i.test(request.headers['content-type'] || ''), 415, 'json_required');
  requireThat(!request.headers['content-encoding'], 415, 'encoding_not_allowed');
  requireThat(!request.headers['content-length'] || Number(request.headers['content-length']) <= limit, 413, 'body_too_large');
  const chunks = []; let size = 0;
  for await (const chunk of request) {
    size += chunk.length;
    requireThat(size <= limit, 413, 'body_too_large');
    chunks.push(chunk);
  }
  let body;
  try { body = JSON.parse(Buffer.concat(chunks).toString('utf8')); } catch { throw fail(400, 'invalid_json'); }
  requireThat(body && typeof body === 'object' && !Array.isArray(body));
  return body;
}
