// Export the application's own pixel art and font; no generated imitation.
import { readFile, writeFile, copyFile, mkdir } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';

const source = new URL('../../native/src/mushroom.rs', import.meta.url);
const assets = new URL('../public/assets/', import.meta.url);
await mkdir(assets, { recursive: true });
const rust = await readFile(source, 'utf8');
const theme = JSON.parse(await readFile(new URL('../../assets/theme.json', import.meta.url), 'utf8'));
const mix = (a, b, percent) => '#' + [1, 3, 5].map(offset => Math.round((parseInt(a.slice(offset, offset + 2), 16) * (100 - percent) + parseInt(b.slice(offset, offset + 2), 16) * percent) / 100).toString(16).padStart(2, '0')).join('');
const { background: bg, indigo: primary, teal: secondary, logo_stem: stem } = theme;
const palette = { R: primary, H: mix(primary, '#ffffff', 24), E: mix(primary, bg, 16), D: mix(primary, bg, 30), W: secondary, V: mix(secondary, '#ffffff', 16), Q: mix(secondary, primary, 15), S: stem, L: mix(stem, '#ffffff', 60), A: mix(stem, bg, 21), B: mix(stem, bg, 12), M: mix(secondary, stem, 46), N: mix(secondary, bg, 35), J: mix(secondary, stem, 70) };
for (const [name, width, height, viewBox] of [['HERO', 80, 72, '0 0 80 72'], ['MINI', 24, 24, '2 3 20 16']]) {
  const declaration = rust.match(new RegExp(`pub const ${name}: [^=]+ = \\[([\\s\\S]*?)\\];`));
  if (!declaration) throw new Error(`Missing ${name} art`);
  const rows = [...declaration[1].matchAll(/b"([.A-Z]+)"/g)].map(match => match[1]);
  if (rows.length !== height || rows.some(row => row.length !== width)) throw new Error(`Invalid ${name} dimensions`);
  const groups = {};
  rows.forEach((row, y) => {
    for (let x = 0; x < row.length;) {
      const pixel = row[x];
      let end = x + 1;
      while (end < row.length && row[end] === pixel) end++;
      if (pixel !== '.') {
        if (!palette[pixel]) throw new Error(`Unknown palette value: ${pixel}`);
        (groups[pixel] ??= []).push(`M${x} ${y}h${end - x}v1H${x}z`);
      }
      x = end;
    }
  });
  const paths = Object.entries(groups).map(([pixel, lines]) => `<path fill="${palette[pixel]}" d="${lines.join('')}"/>`).join('');
  await writeFile(new URL(`mushroom-${name.toLowerCase()}.svg`, assets), `<svg xmlns="http://www.w3.org/2000/svg" viewBox="${viewBox}" shape-rendering="crispEdges">${paths}</svg>\n`);
  if (name === 'MINI') {
    // Transparent spots keep the original mushroom readable as a single-color imprint.
    const silhouette = ['R', 'S'].map(pixel => `<path fill="white" d="${groups[pixel].join('')}"/>`).join('');
    await writeFile(new URL('mushroom-imprint.svg', assets), `<svg xmlns="http://www.w3.org/2000/svg" viewBox="${viewBox}" shape-rendering="crispEdges">${silhouette}</svg>\n`);
  }
}
await copyFile(new URL('../../assets/fonts/Tiny5-Regular.ttf', import.meta.url), new URL('Tiny5-Regular.ttf', assets));
await copyFile(new URL('../../Tiny5-LICENSE.txt', import.meta.url), new URL('Tiny5-LICENSE.txt', assets));
await copyFile(new URL('../../assets/preview.png', import.meta.url), new URL('app-preview.png', assets));
await copyFile(new URL('../../assets/github-social-preview.png', import.meta.url), new URL('social-preview.png', assets));
console.log(`Exported native pixel artwork, font with license and app preview to ${fileURLToPath(assets)}`);
