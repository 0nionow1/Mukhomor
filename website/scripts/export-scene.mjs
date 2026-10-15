// Port of native/visual_build.rs: bake the app's domain-warped pixel field.
// Browsers only interpolate tiny frames; the noise is computed once here.
import { readFile, writeFile } from 'node:fs/promises';
const theme = JSON.parse(await readFile(new URL('../../assets/theme.json', import.meta.url), 'utf8'));
const source = await readFile(new URL('../../native/src/mushroom.rs', import.meta.url), 'utf8');
const rows = [...source.match(/pub const HERO: [^=]+ = \[([\s\S]*?)\];/)[1].matchAll(/b"([.A-Z]+)"/g)].map(match => match[1]);
const rgb = hex => [1, 3, 5].map(i => parseInt(hex.slice(i, i + 2), 16));
const blend = (a, b, t) => a.map((v, i) => Math.round(v + (b[i] - v) * t));
const hash = (x, y, seed) => {
  let n = Math.imul(x, 0x9e3779b9) ^ Math.imul(y, 0x85ebca6b) ^ seed;
  n ^= n >>> 16; n = Math.imul(n, 0x7feb352d); n ^= n >>> 15; n = Math.imul(n, 0x846ca68b);
  return (n ^ (n >>> 16)) >>> 0;
};
const noise = (x, y, seed) => {
  const ix = Math.floor(x), iy = Math.floor(y), fx = x - ix, fy = y - iy;
  const fade = t => t * t * t * (t * (t * 6 - 15) + 10);
  const gradient = (dx, dy) => {
    const px = fx - dx, py = fy - dy;
    return [px, -px, py, -py, (px + py) * Math.SQRT1_2, (px - py) * Math.SQRT1_2, (-px + py) * Math.SQRT1_2, (-px - py) * Math.SQRT1_2][hash(ix + dx, iy + dy, seed) & 7];
  };
  const mix = (a, b, t) => a + (b - a) * t;
  return mix(mix(gradient(0, 0), gradient(1, 0), fade(fx)), mix(gradient(0, 1), gradient(1, 1), fade(fx)), fade(fy));
};
const fbm = (x, y, seed) => {
  let sum = 0, weight = 1, total = 0;
  for (let octave = 0; octave < theme.texture_octaves; octave++) {
    sum += noise(x, y, (seed + octave * 0x17ab21) >>> 0) * weight;
    total += weight; [x, y] = [x * 1.84 - y * .79 + 7.3, x * .79 + y * 1.84 + 3.9]; weight *= .48;
  }
  return sum / total;
};
const width = 144, height = 96, frames = 24, seed = theme.texture_seed;
const data = new Uint8Array(width * height * frames);
for (let frame = 0; frame < frames; frame++) {
  const angle = frame * Math.PI * 2 / frames, dx = Math.sin(angle) * .32, dy = (Math.cos(angle) - 1) * .38;
  for (let y = 0; y < height; y++) for (let x = 0; x < width; x++) {
    const px = x * theme.texture_grain / theme.texture_flow_scale + 3.17;
    const py = y * theme.texture_grain / theme.texture_flow_scale + 7.91;
    const wx = fbm(px * .71 + dx, py * .71 + dy, seed ^ 0x6395dc21);
    const wy = fbm(px * .71 + 13.7 - dy, py * .71 + 4.2 + dx, seed ^ 0x791dc553);
    const value = fbm(px + dx + wx * theme.texture_warp_strength / theme.texture_flow_scale, py + dy + wy * theme.texture_warp_strength / theme.texture_flow_scale, seed);
    const level = Math.round(Math.max(0, Math.min(1, .5 + value * 1.65)) * (theme.texture_levels - 1));
    data[frame * width * height + y * width + x] = Math.round(level * 255 / (theme.texture_levels - 1));
  }
}
const low = rgb(theme.texture_zinc_low), mid = rgb(theme.texture_zinc_mid), high = rgb(theme.texture_zinc_high);
const bg = rgb(theme.background), primary = rgb(theme.indigo), secondary = rgb(theme.teal), stem = rgb(theme.logo_stem);
const accent = blend(rgb(theme.texture_brand_base), primary, theme.texture_indigo_tint / 100);
const smooth = t => t * t * (3 - 2 * t);
const palette = Array.from({ length: 256 }, (_, level) => {
  const t = level / theme.texture_zinc_ceiling;
  const color = level <= theme.texture_zinc_ceiling ? (t < .5 ? blend(low, mid, smooth(t * 2)) : blend(mid, high, smooth((t - .5) * 2))) : blend(high, accent, smooth((level - theme.texture_zinc_ceiling) / (255 - theme.texture_zinc_ceiling)));
  return blend(bg, color, .94);
});
const white = [255, 255, 255];
const colors = { R: primary, H: blend(primary, white, .24), E: blend(primary, bg, .16), D: blend(primary, bg, .3), W: secondary, V: blend(secondary, white, .16), Q: blend(secondary, primary, .15), S: stem, L: blend(stem, white, .6), A: blend(stem, bg, .21), B: blend(stem, bg, .12), M: blend(secondary, stem, .46), N: blend(secondary, bg, .35), J: blend(secondary, stem, .7) };
const scene = { width, height, frames, field: Buffer.from(data).toString('base64'), palette, rows, colors };
await writeFile(new URL('../public/assets/scene-data.js', import.meta.url), `// Baked from the application's pixel field and mushroom artwork.\nwindow.MukhomorScene = ${JSON.stringify(scene)};\n`);
const groups = {};
for (let y = 0; y < height; y++) for (let x = 0; x < width;) {
  const level = data[y * width + x]; let end = x + 1;
  while (end < width && data[y * width + end] === level) end++;
  (groups[level] ??= []).push(`M${x} ${y}h${end-x}v1H${x}z`); x = end;
}
const paths = Object.entries(groups).map(([level, runs]) => `<path fill="rgb(${palette[level].join(',')})" d="${runs.join('')}"/>`).join('');
await writeFile(new URL('../public/assets/field-still.svg', import.meta.url), `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${width} ${height}" preserveAspectRatio="xMidYMid slice" shape-rendering="crispEdges">${paths}</svg>\n`);
console.log(`Baked ${frames} field frames (${width}×${height}) and the original mushroom; no runtime noise computation.`);
