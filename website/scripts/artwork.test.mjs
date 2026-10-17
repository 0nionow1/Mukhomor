import {test} from 'node:test';
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {artwork,backgroundArtwork,toneColors,colorAt} from './mushroom-art.mjs';

function pixels(svg,width,height){
 const result=Array(width*height).fill(null);
 for(const m of svg.matchAll(/<path fill="(#[a-f\d]+)" d="([^"]+)"/gi))for(const run of m[2].matchAll(/M(\d+) (\d+)h(\d+)v1H\d+z/g))for(let x=+run[1];x<+run[1]+ +run[3];x++)result[+run[2]*width+x]=m[1].toLowerCase();
 return result;
}
const fingerprint=rows=>rows.join('').split('').reduce((h,c)=>BigInt.asUintN(64,(h^BigInt(c.charCodeAt(0)))*0x100000001b3n),0xcbf29ce484222325n);
test('Neon tones preserve the approved colors and static exports pixel for pixel',async()=>{
 assert.deepEqual(toneColors,artwork.colors.map(c=>c.rgb));
 for(const[name,art]of[['hero',artwork.hero],['mini',artwork.mini]]){
  const svg=await readFile(new URL(`../public/assets/mushroom-${name}.svg`,import.meta.url),'utf8');
  assert.deepEqual(pixels(svg,art.width,art.height),art.tones.flat().map(colorAt));
  assert.match(svg,/shape-rendering="crispEdges"/);
  assert.doesNotMatch(svg,/<image|filter|gradient|opacity=/i);
 }
 assert.equal(fingerprint(artwork.hero.rows),0x90827ecdbfa06661n);
 assert.equal(fingerprint(artwork.mini.rows),0x5302d2c1b083e085n);
});
test('animated website data uses the same silhouette and tonal colors',async()=>{
 const source=await readFile(new URL('../public/assets/scene-data.js',import.meta.url),'utf8');
 const scene=JSON.parse(source.slice(source.indexOf(' = ')+3).trim().replace(/;$/,''));
 assert.deepEqual(scene.rows,artwork.hero.rows);
 assert.deepEqual(scene.tones,artwork.hero.tones);
 assert.deepEqual(scene.toneColors,toneColors);
 assert.equal(scene.rows.slice(48).join('').split('').filter(c=>c!=='.').length,282);
});
test('all shaded cells retain their semantic material and transparency',()=>{
 for(const art of [artwork.hero,artwork.mini,artwork.compact,artwork.micro]){
  assert.equal(art.rows.length,art.height);assert.equal(art.tones.length,art.height);
  art.rows.forEach((row,y)=>{assert.equal(row.length,art.width);assert.equal(art.tones[y].length,art.width);[...row].forEach((glyph,x)=>{
   const tone=art.tones[y][x];assert.equal(tone===255,glyph==='.');
   if(tone!==255)assert.ok(tone>=14&&tone<14+artwork.colors.length);
  });});
 }
});

test('background layers share one integer grid and meet the animated root anchors',async()=>{
 const art=backgroundArtwork;
 assert.equal(art.width,20);assert.equal(art.height,16);
 assert.equal(art.rows[15],'.'.repeat(20));
 assert.equal(art.rows[14][8],'S');assert.equal(art.rows[14][12],'S');
 assert.doesNotMatch(art.rows.join(''),/[MNJ]/);
 const sprite=await readFile(new URL('../public/assets/mushroom-background.svg',import.meta.url),'utf8');
 assert.match(sprite,/viewBox="0 0 20 16"/);
 assert.deepEqual(pixels(sprite,20,16),art.tones.flat().map(colorAt));
 for(const side of ['left','right']){
  const gutter=await readFile(new URL(`../public/assets/gutter-${side}.svg`,import.meta.url),'utf8');
  const mask=gutter.match(/<g id="pink">([\s\S]*?)<\/g>/)[1];
  assert.deepEqual(pixels(mask,20,16).map(Boolean),art.tones.flat().map(c=>c!==255));
  assert.match(gutter,/translate\(-10 -8\)/);
 }
 const runtime=await readFile(new URL('../public/experience.js',import.meta.url),'utf8');
 assert.match(runtime,/assets\/mushroom-background\.svg/);
 assert.match(runtime,/cy \+ 7 \* scale/);
});
