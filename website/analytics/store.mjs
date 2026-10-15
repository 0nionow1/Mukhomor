import { DatabaseSync } from 'node:sqlite';
import { equal } from './http.mjs';
import { contentStore } from './content-store.mjs';

export const dayOf = time => new Date(time).toISOString().slice(0, 10);
const DAY = 86400000;

export function openStore(path) {
  const db = new DatabaseSync(path, { timeout: 1000, defensive: true });
  db.exec(`
    PRAGMA journal_mode=WAL;
    PRAGMA synchronous=NORMAL;
    PRAGMA journal_size_limit=4194304;
    PRAGMA max_page_count=32768;
    CREATE TABLE IF NOT EXISTS sessions (
      id TEXT PRIMARY KEY, visitor TEXT NOT NULL, country TEXT NOT NULL,
      expires INTEGER NOT NULL, views INTEGER NOT NULL DEFAULT 0, clicks INTEGER NOT NULL DEFAULT 0
    ) STRICT;
    CREATE INDEX IF NOT EXISTS session_expiry ON sessions(expires);
    CREATE TABLE IF NOT EXISTS events (
      session TEXT NOT NULL, id TEXT NOT NULL, expires INTEGER NOT NULL,
      PRIMARY KEY(session,id)
    ) STRICT;
    CREATE INDEX IF NOT EXISTS event_expiry ON events(expires);
    CREATE TABLE IF NOT EXISTS visitors (
      day TEXT NOT NULL, visitor TEXT NOT NULL, country TEXT NOT NULL,
      PRIMARY KEY(day,visitor,country)
    ) STRICT;
    CREATE TABLE IF NOT EXISTS totals (
      day TEXT NOT NULL, country TEXT NOT NULL,
      visits INTEGER NOT NULL DEFAULT 0, pageviews INTEGER NOT NULL DEFAULT 0,
      github INTEGER NOT NULL DEFAULT 0, downloads INTEGER NOT NULL DEFAULT 0,
      PRIMARY KEY(day,country)
    ) STRICT;
    CREATE TABLE IF NOT EXISTS budgets (
      day TEXT PRIMARY KEY, sessions INTEGER NOT NULL DEFAULT 0, events INTEGER NOT NULL DEFAULT 0
    ) STRICT;
    CREATE TABLE IF NOT EXISTS admin_limits (
      scope TEXT PRIMARY KEY, count INTEGER NOT NULL, expires INTEGER NOT NULL
    ) STRICT;
    CREATE TABLE IF NOT EXISTS admin_sessions (
      id TEXT PRIMARY KEY, binding TEXT NOT NULL, version TEXT NOT NULL,
      expires INTEGER NOT NULL, idle INTEGER NOT NULL
    ) STRICT;
    CREATE TABLE IF NOT EXISTS admin_challenges (
      id TEXT PRIMARY KEY, binding TEXT NOT NULL, version TEXT NOT NULL,
      code_hash TEXT NOT NULL, reference TEXT NOT NULL, expires INTEGER NOT NULL,
      attempts INTEGER NOT NULL DEFAULT 0, ready INTEGER NOT NULL DEFAULT 0
    ) STRICT;
  `);
  const statements = new Map();
  const sql = text => {
    if (!statements.has(text)) statements.set(text, db.prepare(text));
    return statements.get(text);
  };
  const transaction = action => {
    db.exec('BEGIN IMMEDIATE');
    try { const result = action(); db.exec('COMMIT'); return result; }
    catch (error) { db.exec('ROLLBACK'); throw error; }
  };
  function budget(day, field, cap) {
    sql('INSERT OR IGNORE INTO budgets(day) VALUES (?)').run(day);
    return sql(`UPDATE budgets SET ${field}=${field}+1 WHERE day=? AND ${field}<?`).run(day, cap).changes === 1;
  }
  function adminBudgets(scopes, now) {
    sql('DELETE FROM admin_limits WHERE expires<=?').run(now);
    let wait = 0;
    for (const [scope, maximum] of scopes) {
      const state = sql('SELECT count,expires FROM admin_limits WHERE scope=?').get(scope);
      if (state && state.count >= maximum) wait = Math.max(wait, state.expires - now);
    }
    if (wait) return wait;
    if (sql('SELECT count(*) AS n FROM admin_limits').get().n >= 5000) return 60000;
    for (const [scope, , expires] of scopes) sql(`INSERT INTO admin_limits(scope,count,expires) VALUES (?,1,?)
      ON CONFLICT(scope) DO UPDATE SET count=count+1`).run(scope, expires);
    return 0;
  }
  return {
    ...contentStore(db, transaction),
    close: () => db.close(),
    reserveContentWrite(now, importing=false) {
      return transaction(()=>adminBudgets(importing ? [['cms-import',10,now+3600000]] : [['cms-write',60,now+60000]],now));
    },
    cleanup(now) {
      transaction(() => {
        sql('DELETE FROM events WHERE expires<=?').run(now);
        sql('DELETE FROM sessions WHERE expires<=?').run(now);
        sql('DELETE FROM visitors WHERE day<=?').run(dayOf(now - 35 * DAY));
        sql('DELETE FROM totals WHERE day<=?').run(dayOf(now - 395 * DAY));
        sql('DELETE FROM budgets WHERE day<?').run(dayOf(now - 2 * DAY));
        sql('DELETE FROM admin_limits WHERE expires<=?').run(now);
        sql('DELETE FROM admin_sessions WHERE expires<=? OR idle<=?').run(now, now);
        sql('DELETE FROM admin_challenges WHERE expires<=?').run(now);
      });
    },
    reserveAdminAttempt(ipHash, now) {
      return transaction(() => {
        const dayEnd = Date.parse(dayOf(now) + 'T00:00:00Z') + DAY;
        const scopes = [[`ip:${ipHash}`, 5, now + 15 * 60000], ['global', 20, now + 15 * 60000], ['daily', 100, dayEnd]];
        return adminBudgets(scopes, now);
      });
    },
    reserveAdminDelivery(now) {
      return transaction(() => adminBudgets([
        ['otp-send:minute', 1, now + 60000], ['otp-send:hour', 5, now + 3600000],
        ['otp-send:day', 20, Date.parse(dayOf(now) + 'T00:00:00Z') + DAY]
      ], now));
    },
    addAdminChallenge(challenge, now) {
      transaction(() => {
        sql('DELETE FROM admin_challenges WHERE expires<=?').run(now);
        sql('INSERT INTO admin_challenges(id,binding,version,code_hash,reference,expires) VALUES (?,?,?,?,?,?)')
          .run(challenge.id, challenge.binding, challenge.version, challenge.codeHash, challenge.reference, challenge.expires);
        sql('DELETE FROM admin_challenges WHERE id NOT IN (SELECT id FROM admin_challenges ORDER BY expires DESC, rowid DESC LIMIT 8)').run();
      });
    },
    markAdminChallengeReady(id, now) { return sql('UPDATE admin_challenges SET ready=1 WHERE id=? AND expires>?').run(id, now).changes === 1; },
    getAdminChallenge(id, now) { return sql('SELECT * FROM admin_challenges WHERE id=? AND expires>? AND ready=1').get(id, now); },
    deleteAdminChallenge(id) { sql('DELETE FROM admin_challenges WHERE id=?').run(id); },
    consumeAdminChallenge({ id, binding, version, codeHash }, now) {
      return transaction(() => {
        const state = sql('SELECT * FROM admin_challenges WHERE id=? AND expires>? AND ready=1').get(id, now);
        if (!state || !equal(binding, state.binding) || !equal(version, state.version)) return 'expired';
        if (!equal(codeHash, state.code_hash)) {
          if (state.attempts >= 4) { sql('DELETE FROM admin_challenges WHERE id=?').run(id); return 'expired'; }
          sql('UPDATE admin_challenges SET attempts=attempts+1 WHERE id=?').run(id);
          return 'invalid';
        }
        sql('DELETE FROM admin_challenges WHERE id=?').run(id);
        return 'accepted';
      });
    },
    addAdminSession(session, now) {
      transaction(() => {
        sql('DELETE FROM admin_sessions WHERE expires<=? OR idle<=?').run(now, now);
        sql('INSERT INTO admin_sessions(id,binding,version,expires,idle) VALUES (?,?,?,?,?)')
          .run(session.id, session.binding, session.version, session.expires, session.idle);
        sql('DELETE FROM admin_sessions WHERE id NOT IN (SELECT id FROM admin_sessions ORDER BY expires DESC, rowid DESC LIMIT 8)').run();
      });
    },
    getAdminSession(id, now) { return sql('SELECT * FROM admin_sessions WHERE id=? AND expires>? AND idle>?').get(id, now, now); },
    touchAdminSession(id, idle) { sql('UPDATE admin_sessions SET idle=? WHERE id=?').run(idle, id); },
    deleteAdminSession(id) { sql('DELETE FROM admin_sessions WHERE id=?').run(id); },
    addSession(session, now, cap) {
      return transaction(() => {
        if (!budget(dayOf(now), 'sessions', cap)) return false;
        sql('INSERT INTO sessions(id,visitor,country,expires) VALUES (?,?,?,?)')
          .run(session.id, session.visitor, session.country, session.expires);
        return true;
      });
    },
    record(sessionId, event, now, cap) {
      return transaction(() => {
        const session = sql('SELECT * FROM sessions WHERE id=? AND expires>?').get(sessionId, now);
        if (!session) return 401;
        if (sql('SELECT 1 FROM events WHERE session=? AND id=?').get(sessionId, event.id)) return 204;
        if (event.kind !== 'page_view' && !session.views) return 409;
        if ((event.kind === 'page_view' && session.views >= 40) || (event.kind !== 'page_view' && session.clicks >= 10)) return 429;
        const day = dayOf(now);
        if (!budget(day, 'events', cap)) return 503;
        const view = Number(event.kind === 'page_view');
        const visit = Number(view && !session.views);
        const download = Number(event.kind === 'download');
        const github = Number(event.kind === 'github' || download);
        sql('INSERT INTO events(session,id,expires) VALUES (?,?,?)').run(sessionId, event.id, session.expires);
        sql('UPDATE sessions SET views=views+?, clicks=clicks+? WHERE id=?').run(view, github, sessionId);
        sql(`INSERT INTO totals(day,country,visits,pageviews,github,downloads) VALUES (?,?,?,?,?,?)
          ON CONFLICT(day,country) DO UPDATE SET visits=visits+excluded.visits,
          pageviews=pageviews+excluded.pageviews, github=github+excluded.github, downloads=downloads+excluded.downloads`)
          .run(day, session.country, visit, view, github, download);
        sql('INSERT OR IGNORE INTO visitors(day,visitor,country) VALUES (?,?,?)').run(day, session.visitor, session.country);
        return 204;
      });
    },
    report(days = 30, now = Date.now()) {
      if (!Number.isInteger(days) || days < 1 || days > 35) throw new Error('Report days must be 1..35');
      return readReport(db, days, now);
    }
  };
}

export function readReport(db, days = 30, now = Date.now()) {
  if (!Number.isInteger(days) || days < 1 || days > 35) throw new Error('Report days must be 1..35');
  const from = dayOf(now - (days - 1) * DAY), to = dayOf(now);
  const totals = db.prepare(`SELECT coalesce(sum(visits),0) AS visits, coalesce(sum(pageviews),0) AS pageviews,
    coalesce(sum(github),0) AS github, coalesce(sum(downloads),0) AS downloads FROM totals WHERE day BETWEEN ? AND ?`).get(from, to);
  const unique = db.prepare('SELECT count(DISTINCT visitor) AS n FROM visitors WHERE day BETWEEN ? AND ?').get(from, to).n;
  const countries = db.prepare(`SELECT country, sum(visits) AS visits, sum(pageviews) AS pageviews,
    sum(github) AS github, sum(downloads) AS downloads,
    (SELECT count(DISTINCT visitor) FROM visitors v WHERE v.country=t.country AND day BETWEEN ? AND ?) AS uniqueVisitors
    FROM totals t WHERE day BETWEEN ? AND ? GROUP BY country ORDER BY visits DESC, country`).all(from, to, from, to);
  const daily = db.prepare(`SELECT day, sum(visits) AS visits, sum(pageviews) AS pageviews, sum(github) AS github,
    sum(downloads) AS downloads, (SELECT count(DISTINCT visitor) FROM visitors v WHERE v.day=t.day) AS uniqueVisitors
    FROM totals t WHERE day BETWEEN ? AND ? GROUP BY day ORDER BY day`).all(from, to);
  return { from, to, timezone: 'UTC', uniqueVisitors: unique, ...totals, countries, daily,
    attribution: 'IP Geolocation by DB-IP (https://db-ip.com), CC BY 4.0, when using the IP deployment template.' };
}
