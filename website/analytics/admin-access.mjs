// Local operator utility. Never expose this file through HTTP.
import { hashPassword } from './admin.mjs';
import { existsSync } from 'node:fs';
import { isAbsolute } from 'node:path';

try {
  const [command, path] = process.argv.slice(2);
  if (command === 'password' && !path) {
    if (process.stdin.isTTY) throw new Error('Read the password from a hidden prompt through stdin, never a command argument.');
    let input = '';
    for await (const chunk of process.stdin) {
      input += chunk;
      if (Buffer.byteLength(input) > 514) throw new Error('Password too long.');
    }
    const hash = await hashPassword(input.replace(/\r?\n$/, '')); input = '';
    process.stdout.write(hash + '\n');
  } else if (['reset-limits', 'revoke-sessions'].includes(command) && path && isAbsolute(path) && existsSync(path)) {
    const { DatabaseSync } = await import('node:sqlite');
    const db = new DatabaseSync(path, { timeout: 1000, defensive: true });
    try {
      db.exec(command === 'reset-limits' ? 'DELETE FROM admin_limits' : 'BEGIN IMMEDIATE; DELETE FROM admin_sessions; DELETE FROM admin_challenges; COMMIT;');
      console.log(command === 'reset-limits' ? 'Admin login limits reset.' : 'Admin sessions revoked.');
    } finally { db.close(); }
  } else throw new Error('Usage: admin-access.mjs password | reset-limits /absolute/existing/database | revoke-sessions /absolute/existing/database');
} catch (error) { console.error(error.message); process.exitCode = 1; }
