import assert from 'node:assert/strict';
import { readFile, readdir, stat } from 'node:fs/promises';
import { resolve, dirname, relative } from 'node:path';
import { fileURLToPath } from 'node:url';
import { Script } from 'node:vm';
import { createHash } from 'node:crypto';
import { syncChangelog } from './changelog.mjs';
import { syncSeo } from './seo.mjs';
import { syncContent } from './content.mjs';

await syncChangelog(true);
await syncSeo(true);
await syncContent(true);

const root = fileURLToPath(new URL('../public/', import.meta.url));
const walk = async directory => (await Promise.all((await readdir(directory, { withFileTypes: true })).map(async item => item.isDirectory() ? walk(resolve(directory, item.name)) : resolve(directory, item.name)))).flat();
const files = await walk(root);
const html = new Map();
let checks = 0;
const check = (value, message) => { assert(value, message); checks++; };
for (const file of files.filter(path => path.endsWith('.html'))) {
  const text = await readFile(file, 'utf8');
  html.set(file, { text, ids: [...text.matchAll(/\bid="([^"]+)"/g)].map(match => match[1]) });
}
for (const [file, { text, ids }] of html) {
  check(ids.length === new Set(ids).size, `Duplicate IDs: ${relative(root, file)}`);
  check(/<html lang="ru">/.test(text), 'Missing page language');
  check((text.match(/<h1\b/g) || []).length === 1, 'Expected one h1');
  check(/name="viewport"/.test(text), 'Missing viewport');
  check((text.match(/rel="canonical"/g) || []).length === 1, 'Expected one canonical URL');
  const structured = [...text.matchAll(/<script type="application\/ld\+json">([\s\S]*?)<\/script>/g)];
  check(structured.length === 1 && JSON.parse(structured[0][1])['@graph'].some(item => item['@type'] === 'SoftwareApplication'), 'Missing project entity');
  for (const match of text.matchAll(/\b(?:href|src)="([^"]+)"/g)) {
    const url = match[1];
    if (/^https:\/\//.test(url)) continue;
    check(!url.startsWith('/'), `Root-relative URL breaks project hosting: ${url}`);
    const [pathAndQuery, hash] = url.split('#');
    const pathname = pathAndQuery.split('?')[0];
    const target = pathname ? resolve(dirname(file), pathname + (pathname.endsWith('/') ? 'index.html' : '')) : file;
    check(target.startsWith(root), `URL leaves public folder: ${url}`);
    check((await stat(target)).isFile(), `Missing target: ${url}`);
    if (hash) check(html.get(target)?.ids.includes(hash), `Missing anchor: ${url}`);
  }
  for (const match of text.matchAll(/<img\b[^>]*>/g)) check(/\balt="[^"]*"/.test(match[0]), 'Image without alt');
  for (const match of text.matchAll(/<a\b[^>]*target="_blank"[^>]*>/g)) check(/rel="noopener noreferrer"/.test(match[0]), 'External link needs rel');
  check(!/\bMycelium\b|(?:^|[\s>])обход|блокиров|российск|русских пользовател|ТСПУ|\bDPI\b/i.test(text), 'Unexpected wording in public copy');
}
for (const file of files.filter(path => path.endsWith('.css'))) {
  const css = await readFile(file, 'utf8');
  for (const match of css.matchAll(/url\(['"]?([^)'"\s]+)['"]?\)/g)) {
    if (match[1].startsWith('#')) {
      const stylesheet = relative(root, file).replaceAll('\\', '/');
      const consumers = [...html.values()].filter(page => [...page.text.matchAll(/\bhref="([^"]+)"/g)].some(link=>link[1].split(/[?#]/)[0]===stylesheet));
      check(consumers.length && consumers.every(page => page.ids.includes(match[1].slice(1))), `Missing inline SVG reference: ${match[1]}`);
    } else check((await stat(resolve(dirname(file), match[1].split(/[?#]/)[0]))).isFile(), `Missing CSS asset: ${match[1]}`);
  }
}
for (const file of files.filter(path => path.endsWith('.js'))) new Script(await readFile(file, 'utf8'));
check(files.some(file => file.endsWith('.nojekyll')), 'Missing .nojekyll');
check(files.some(file => file.endsWith('Tiny5-LICENSE.txt')), 'Missing font license');
const home = html.get(resolve(root, 'index.html')).text;
check(home.includes('Mycelia'), 'Use the requested protocol name');
check(home.includes('В разработке') && home.includes('Свой VPS или P2P.') && home.includes('Tor для TCP') && home.includes('.i2p'), 'Connection availability must be explicit');
check(home.includes('data-enter') && home.includes('id="story"'), 'Read more transition needs a real anchor fallback');
// Theme variants are selected dynamically, so ordinary href/src checks miss them.
const shots = resolve(root, 'assets/screenshots');
const manifest = JSON.parse(await readFile(resolve(shots, 'manifest.json'), 'utf8'));
for (const entry of manifest.screenshots) {
  check(/^[a-z-]+\.png$/.test(entry.file), 'Invalid screenshot filename');
  const png = await readFile(resolve(shots, entry.file));
  check(png.subarray(0, 8).equals(Buffer.from([137, 80, 78, 71, 13, 10, 26, 10])), 'Invalid screenshot PNG');
  check(png.readUInt32BE(16) === entry.width && png.readUInt32BE(20) === entry.height, 'Screenshot dimensions differ from manifest');
  check(createHash('sha256').update(png).digest('hex') === entry.sha256, 'Screenshot differs from rendered manifest');
}
for (const screen of ['connection', 'rules', 'presets', 'appearance']) {
  for (const suffix of ['', '-paper', '-glacier']) check(manifest.screenshots.some(entry => entry.file === `${screen}${suffix}.png`), `Missing gallery variant: ${screen}${suffix}`);
}
console.log(`PASS: ${checks} checks; ${files.length} public files. Relative links, anchors, assets, metadata, content constraints and JavaScript syntax verified.`);
