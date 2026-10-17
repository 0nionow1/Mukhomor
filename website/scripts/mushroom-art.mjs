// Shared approved artwork: keep semantic masks separate from the tonal pixels.
import { readFile } from 'node:fs/promises';
export const artwork = JSON.parse(await readFile(new URL('../../assets/mushroom-art.json', import.meta.url), 'utf8'));
export const theme = JSON.parse(await readFile(new URL('../../assets/theme.json', import.meta.url), 'utf8'));
export const rgb = hex => [1, 3, 5].map(i => parseInt(hex.slice(i, i + 2), 16));
export const hex = c => '#' + c.map(v => v.toString(16).padStart(2, '0')).join('');
export const toneColors = artwork.colors.map(({rgb: reference, family}) => {
 const base = [[255,82,185],[22,245,209],[243,255,244]][family];
 const themed = rgb([theme.indigo, theme.teal, theme.logo_stem][family]);
 return reference.map((r,i) => r <= base[i] ? Math.floor((themed[i]*r+Math.floor(base[i]/2))/Math.max(1,base[i])) : themed[i]+Math.floor(((255-themed[i])*(r-base[i])+Math.floor((255-base[i])/2))/Math.max(1,255-base[i])));
});
export const colorAt = tone => tone === 255 ? null : hex(toneColors[tone - 14]);
// Background emitters have a fixed 20×16 contract: the stem ends at row 15,
// where the animated branches begin. Use the same pixels in both layers.
const backgroundRows = [3,4,5,6,7,8,9,10,12,13,14,17,18,19,20,null];
export const backgroundArtwork = {
 width:20,height:16,
 rows:backgroundRows.map((source,y)=>{
  if(source===null)return '.'.repeat(20);
  const row=[...artwork.compact.rows[source].slice(2,22)];
  if(y>=6&&y<=8){row[0]='.';row[19]='.';}
  return row.join('');
 }),
 tones:backgroundRows.map((source,y)=>{
  if(source===null)return Array(20).fill(255);
  const row=artwork.compact.tones[source].slice(2,22);
  if(y>=6&&y<=8){row[0]=255;row[19]=255;}
  return row;
 }),
};
export function paths(art, monochrome = false) {
 const groups = new Map();
 for(let y=0;y<art.height;y++)for(let x=0;x<art.width;){
  const glyph=art.rows[y][x],color=monochrome ? (glyph !== '.' && !'WVQ'.includes(glyph) ? '#ffffff' : null) : colorAt(art.tones[y][x]);
  let end=x+1;while(end<art.width&&(monochrome ? (art.rows[y][end]!=='.'&&!'WVQ'.includes(art.rows[y][end])?'#ffffff':null) : colorAt(art.tones[y][end]))===color)end++;
  if(color){if(!groups.has(color))groups.set(color,[]);groups.get(color).push(`M${x} ${y}h${end-x}v1H${x}z`);}x=end;
 }
 return [...groups].map(([color,runs])=>`<path fill="${color}" d="${runs.join('')}"/>`).join('');
}
export function svg(art, monochrome = false) { return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${art.width} ${art.height}" shape-rendering="crispEdges">${paths(art,monochrome)}</svg>\n`; }
