// Reuse the native MINI pixel grid, with fixed positions independent of page size.
import { readFile, writeFile } from 'node:fs/promises';

import { backgroundArtwork } from './mushroom-art.mjs';
const rows = backgroundArtwork.rows;
if (rows.length !== 16 || rows.some(row => row.length !== 20)) throw new Error('Invalid background emitter grid');
const paths = { body: [], spots: [] };
rows.forEach((row, y) => {
  for (let x = 0; x < row.length;) {
    const pixel = row[x];
    let end = x + 1;
    while (end < row.length && row[end] === pixel) end++;
    if (pixel !== '.') {
      paths['WVQ'.includes(pixel) ? 'spots' : 'body'].push(`M${x} ${y}h${end - x}v1H${x}z`);
    }
    x = end;
  }
});

// Similar brightness to the former grey imprints. The spots swap the two accents.
const pink = '#37232f', teal = '#1b302f';
const definitions = [['pink', pink, teal], ['teal', teal, pink]].map(([id, body, spots]) =>
  `<g id="${id}"><path fill="${body}" d="${paths.body.join('')}"/><path fill="${spots}" d="${paths.spots.join('')}"/></g>`
).join('');
// x/y centers, integer pixel scale, color, opacity, horizontal direction.
const layouts = {
  left: [
    [92, 100, 7, 'pink', .8, 1], [198, 308, 3, 'teal', .6, -1],
    [130, 550, 11, 'teal', .65, 1], [202, 782, 5, 'pink', .75, -1],
    [72, 1032, 4, 'pink', .55, 1], [166, 1300, 8, 'teal', .7, 1]
  ],
  right: [
    [186, 168, 4, 'teal', .65, -1], [126, 420, 10, 'pink', .75, 1],
    [210, 660, 2, 'pink', .6, -1], [102, 880, 7, 'teal', .7, 1],
    [192, 1108, 3, 'teal', .55, 1], [116, 1330, 9, 'pink', .65, -1]
  ]
};
const tileStyles = ['.gutter-art { --gutter-tile-height: 1440; }'];
for (const [side, layout] of Object.entries(layouts)) {
  const shapes = layout.map(([x, y, scale, color, opacity, flip]) =>
    `<use href="#${color}" opacity="${opacity}" transform="translate(${x} ${y}) scale(${scale * flip} ${scale}) translate(-10 -8)"/>`
  ).join('');
  await writeFile(new URL(`../public/assets/gutter-${side}.svg`, import.meta.url),
    `<svg xmlns="http://www.w3.org/2000/svg" width="280" height="1440" viewBox="0 0 280 1440" shape-rendering="crispEdges"><defs>${definitions}</defs>${shapes}</svg>\n`);
  // Occupied pixel rows let the page hide a partial final mushroom without moving any others.
  const occupied = rows.map((row,y)=>/[^.]/.test(row)?y:-1).filter(y=>y>=0);
  const bands = layout.map(([, y, scale]) => `${y + (Math.min(...occupied) - 8) * scale} ${y + (Math.max(...occupied) + 1 - 8) * scale}`).join(', ');
  // Let the live decoration choose from every size, using the exact tile geometry.
  const sources = layout.map(([x, y, scale, , , flip]) => `${x} ${y} ${scale} ${flip}`).join(', ');
  tileStyles.push(`.gutter-art-${side} { background-image: url('assets/gutter-${side}.svg?v=background-2'); --gutter-bands: ${bands}; --gutter-sources: ${sources}; }`);
}
const cssPath = new URL('../public/scene.css', import.meta.url);
const css = await readFile(cssPath, 'utf8');
const marker = /(\/\* gutter-tiles:start[^\n]*\n)[\s\S]*?(\/\* gutter-tiles:end \*\/)/;
if (!marker.test(css)) throw new Error('Missing gutter CSS markers');
await writeFile(cssPath, css.replace(marker, (_, start, end) => start + tileStyles.join('\n') + '\n' + end));
console.log('Exported two static gutter tiles from the native MINI pixel grid.');
