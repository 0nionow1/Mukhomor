// Render the actual Slint UI in isolation; publish only this explicit allowlist.
import { spawnSync } from 'node:child_process';
import { copyFile, mkdir, readFile, writeFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { createHash } from 'node:crypto';

const root = new URL('../../', import.meta.url);
const rendered = new URL('target/website-screenshots/', root);
const destination = new URL('../public/assets/screenshots/', import.meta.url);
const result = spawnSync('cargo', ['run', '--manifest-path', 'ui/Cargo.toml', '--locked', '--offline', '--features', 'preview', '--bin', 'mukhomor-preview', '--', '--render', fileURLToPath(rendered)], { cwd: fileURLToPath(root), stdio: 'inherit', windowsHide: true });
if (result.error) throw result.error;
if (result.status !== 0) process.exit(result.status || 1);
await mkdir(destination, { recursive: true });
const files = { connection: 'website-connection', import: 'import', rules: 'website-rules', presets: 'presets', settings: 'settings', appearance: 'appearance' };
for (const screen of ['connection', 'rules', 'presets', 'appearance']) {
  for (const theme of ['paper', 'glacier']) files[`${screen}-${theme}`] = `website-${screen}-${theme}`;
}
const manifest = { source: 'ui/src/preview.rs — isolated Slint renderer, synthetic data only', screenshots: [] };
for (const [name, fixture] of Object.entries(files)) {
  const source = new URL(`${fixture}.png`, rendered);
  const bytes = await readFile(source);
  await copyFile(source, new URL(`${name}.png`, destination));
  manifest.screenshots.push({ file: `${name}.png`, fixture, width: bytes.readUInt32BE(16), height: bytes.readUInt32BE(20), sha256: createHash('sha256').update(bytes).digest('hex') });
}
await writeFile(new URL('manifest.json', destination), JSON.stringify(manifest, null, 2) + '\n');
// Reserve the actual image ratio even when the native window size changes.
for (const page of ['index', 'docs']) {
  const file = new URL(`../public/${page}.html`, import.meta.url);
  const html = await readFile(file, 'utf8');
  const updated = html.replace(/<img\b[^>]*\bsrc="assets\/screenshots\/([^"/]+)"[^>]*>/g, (tag, name) => {
    const entry = manifest.screenshots.find(item => item.file === name);
    if (!entry) throw new Error(`Unknown screenshot: ${name}`);
    return tag.replace(/\bwidth="\d+"/, `width="${entry.width}"`).replace(/\bheight="\d+"/, `height="${entry.height}"`);
  });
  if (updated !== html) await writeFile(file, updated);
}
console.log(`Exported ${manifest.screenshots.length} original application screenshots.`);
