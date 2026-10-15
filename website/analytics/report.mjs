import { DatabaseSync } from 'node:sqlite';
import { parseArgs } from 'node:util';
import { readReport } from './store.mjs';

const { values } = parseArgs({ options: { db: { type: 'string' }, days: { type: 'string', default: '30' } } });
if (!values.db) throw new Error('Usage: node report.mjs --db /private/path/analytics.sqlite --days 30');
const db = new DatabaseSync(values.db, { readOnly: true, timeout: 1000 });
try { console.log(JSON.stringify(readReport(db, Number(values.days)), null, 2)); }
finally { db.close(); }
