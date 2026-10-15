import assert from 'node:assert/strict';
import { readFile, writeFile } from 'node:fs/promises';
import { pathToFileURL } from 'node:url';

const htmlPath = new URL('../public/changelog.html', import.meta.url);
const escape = value => value.replace(/[&<>"']/g, char => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[char]);

export async function syncChangelog(checkOnly = false) {
  const data = JSON.parse(await readFile(new URL('../changelog.json', import.meta.url), 'utf8'));
  const managed = JSON.parse(await readFile(new URL('../content.json', import.meta.url), 'utf8'));
  data.releases=managed.releases.filter(item=>item.visible).sort((a,b)=>Date.parse(b.publishedAt)-Date.parse(a.publishedAt));
  const history = await readFile(new URL('../../CHANGELOG.md', import.meta.url), 'utf8');
  assert(Array.isArray(data.releases), 'Expected published releases');
  const validateCopy = entry => {
    assert(typeof entry.title === 'string' && entry.title.trim(), 'Missing release title');
    assert(Array.isArray(entry.changes) && entry.changes.length, 'Missing release changes');
    for (const change of entry.changes) assert(typeof change === 'string' && change.trim(), 'Empty change');
  };
  const items = [];
  const list = entry => `<h3>${escape(entry.title)}</h3><ul>${entry.changes.map(change => `<li>${escape(change)}</li>`).join('')}</ul>`;
  if (data.unreleased) {
    validateCopy(data.unreleased);
    assert(history.includes('## [Unreleased]'), 'Add pending changes to CHANGELOG.md');
    items.push(`<article class="release-entry release-pending" id="unreleased" aria-labelledby="unreleased-title"><div class="release-meta"><span class="status development-status">Ещё не выпущено</span><h2 id="unreleased-title">Следующее обновление</h2></div><div class="release-notes">${list(data.unreleased)}</div></article>`);
  }
  items.push('<div id="published-history">');
  for (const release of data.releases) {
    validateCopy(release);
    assert(/^\d+\.\d+\.\d+(?:-[A-Za-z0-9.-]+)?$/.test(release.version), 'Expected a release version');
    assert(/^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ$/.test(release.publishedAt) && Number.isFinite(Date.parse(release.publishedAt)), 'Use the confirmed GitHub published_at timestamp');
    const date = new Intl.DateTimeFormat('ru-RU', { day: 'numeric', month: 'long', year: 'numeric', timeZone: 'Europe/Moscow' }).format(new Date(release.publishedAt));
    const version = release.version;
    items.push(`<article class="release-entry" id="v${version}" aria-labelledby="v${version}-title"><div class="release-meta"><span class="status ${release.status==='stable'?'available-status':'development-status'}">${release.status==='stable'?'Stable':'В проверке'}</span><h2 id="v${version}-title"><a href="#v${version}">${version}</a></h2><time datetime="${release.publishedAt}">${date}</time></div><div class="release-notes">${list(release)}<a class="text-link release-source" href="https://github.com/0nionow1/Mukhomor/releases/tag/v${version}" target="_blank" rel="noopener noreferrer">Релиз на GitHub <span aria-hidden="true">↗</span></a></div></article>`);
  }
  items.push('</div>');
  const original = await readFile(htmlPath, 'utf8');
  const marker = /<!-- changelog:start -->[\s\S]*?<!-- changelog:end -->/;
  assert(marker.test(original), 'Missing changelog template markers');
  const output = original.replace(marker, `<!-- changelog:start -->\n${items.join('\n')}\n<!-- changelog:end -->`);
  if (checkOnly) assert.equal(original, output, 'Changelog is stale: run node website/scripts/changelog.mjs');
  else await writeFile(htmlPath, output);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  await syncChangelog(process.argv.includes('--check'));
  console.log(process.argv.includes('--check') ? 'PASS: changelog is up to date.' : 'Updated website/public/changelog.html');
}
