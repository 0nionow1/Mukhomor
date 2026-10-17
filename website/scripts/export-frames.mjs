import { writeFile } from 'node:fs/promises';

// Like the hero's roots: long stair-stepped strands split into shorter roots.
// One source cell is a 4px square. Never smooth or stretch the pixel artwork.
const assets = new URL('../public/assets/', import.meta.url);
const stepped = points => {
  let [x, y] = points[0];
  let d = `M${x + .5} ${y + .5}`;
  for (const [tx, ty] of points.slice(1)) {
    const sx = x, sy = y, count = Math.max(Math.abs(tx - sx), Math.abs(ty - sy));
    for (let step = 1; step <= count; step++) {
      const nx = Math.round(sx + (tx - sx) * step / count);
      const ny = Math.round(sy + (ty - sy) * step / count);
      if (nx !== x) d += `H${nx + .5}`;
      if (ny !== y) d += `V${ny + .5}`;
      [x, y] = [nx, ny];
    }
  }
  return d;
};
const strand = (points, color) => `<path d="${stepped(points)}" fill="none" stroke="${color}" stroke-width="1" stroke-linecap="square"/>`;
const edge = (line, shade) =>
  strand([[6,2],[14,2],[20,4],[28,3],[38,2],[46,4],[58,3],[66,1],[74,2]], line) +
  strand([[20,4],[25,5],[31,5]], shade) +
  strand([[38,2],[43,1],[50,0]], shade) +
  strand([[58,3],[63,5],[69,5]], shade);
const corner = (line, shade) =>
  strand([[6,2],[4,2],[3,3],[2,4],[2,6]], line) +
  strand([[4,2],[2,1],[0,1]], shade) +
  strand([[2,4],[1,5],[0,5]], shade) +
  strand([[3,3],[4,4],[5,5]], shade);
const smallEdge = (line, shade) =>
  strand([[4,1],[9,1],[13,2],[18,1],[26,2],[33,1],[39,2],[44,1]], line) +
  strand([[13,2],[16,3],[21,3]], shade) +
  strand([[33,1],[36,0],[41,0]], shade);
const smallCorner = (line, shade) =>
  strand([[4,1],[2,1],[1,2],[1,4]], line) +
  strand([[2,1],[1,0],[0,0]], shade) +
  strand([[1,2],[2,3],[3,3]], shade);
const palettes = {
  panel: ['#1a1122', '#694456', '#422d3d'],
  quiet: ['#120c18', '#694456', '#422d3d'],
  pink: ['#21121e', '#86556e', '#533549'],
  action: ['#1a1122', '#80516a', '#422d3d'],
  signal: ['#12342f', '#16f5d1', '#397d6e'],
  pressed: ['#16f5d1', '#127c70', '#209a89']
};
for (const [name, [fill, line, shade]] of Object.entries(palettes)) {
  for (const compact of [false, true]) {
    if (!compact && ['signal', 'pressed'].includes(name)) continue;
    const size = compact ? 48 : 80;
    const surface = compact
      ? 'M3 1H45V2H46V3H47V45H46V46H45V47H3V46H2V45H1V3H2V2H3Z'
      : 'M6 2H74V3H76V4H77V6H78V74H77V76H76V77H74V78H6V77H4V76H3V74H2V6H3V4H4V3H6Z';
    const sides = [0,90,180,270].map(angle => `<g transform="rotate(${angle} ${size / 2} ${size / 2})">${(compact ? smallEdge : edge)(line, shade)}${(compact ? smallCorner : corner)(line, shade)}</g>`).join('');
    const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="${size * 4}" height="${size * 4}" viewBox="0 0 ${size} ${size}" shape-rendering="crispEdges"><path fill="${fill}" d="${surface}"/>${sides}</svg>\n`;
    await writeFile(new URL(`pixel-frame-${name}${compact ? '-compact' : ''}.svg`, assets), svg);
  }
}
const root = '#694456', tip = '#422d3d';
// Same nine-slice geometry as action buttons, with no surface fill anywhere.
const pulseSides = [0,90,180,270].map(angle => `<g transform="rotate(${angle} 24 24)">${smallEdge('#16f5d1', '#397d6e')}${smallCorner('#16f5d1', '#397d6e')}</g>`).join('');
await writeFile(new URL('pixel-frame-pulse-compact.svg', assets), `<svg xmlns="http://www.w3.org/2000/svg" width="192" height="192" viewBox="0 0 48 48" shape-rendering="crispEdges">${pulseSides}</svg>\n`);
// Exact top-edge mask for a short signal on the recommended release frame.
await writeFile(new URL('pixel-frame-edge-mask.svg', assets), `<svg xmlns="http://www.w3.org/2000/svg" width="272" height="24" viewBox="6 0 68 6" shape-rendering="crispEdges">${edge('#fff', '#fff')}</svg>\n`);
const rule = strand([[-1,6],[6,6],[12,4],[18,3],[25,5],[32,6],[41,5],[48,7],[55,8],[62,6],[68,5],[74,6],[80,6]], root) +
  strand([[18,3],[23,2],[29,1],[34,1]], tip) +
  strand([[32,6],[38,8],[43,10],[48,11]], tip) +
  strand([[62,6],[67,3],[73,2],[78,3]], tip) +
  strand([[43,10],[47,9],[52,9]], tip);
await writeFile(new URL('pixel-mycelium-rule.svg', assets), `<svg xmlns="http://www.w3.org/2000/svg" width="320" height="48" viewBox="0 0 80 12" shape-rendering="crispEdges">${rule}</svg>\n`);
// A separate low profile root keeps compact dividers at the same 4px scale.
const compactRule = strand([[-1,2],[7,2],[13,1],[19,2],[27,2],[34,1],[41,2],[48,2],[54,1],[62,2],[69,1],[74,2],[80,2]], root) +
  strand([[13,1],[17,0],[22,0]], tip) +
  strand([[41,2],[46,3],[52,3]], tip) +
  strand([[69,1],[73,0],[78,0]], tip);
await writeFile(new URL('pixel-mycelium-rule-compact.svg', assets), `<svg xmlns="http://www.w3.org/2000/svg" width="320" height="16" viewBox="0 0 80 4" shape-rendering="crispEdges">${compactRule}</svg>\n`);
await writeFile(new URL('pixel-corner.svg', assets), `<svg xmlns="http://www.w3.org/2000/svg" width="32" height="32" viewBox="0 0 8 8" shape-rendering="crispEdges">${corner(root, tip)}</svg>\n`);
console.log('Exported branching rose roots on a consistent 4px pixel grid.');
