// Export the application's own pixel art and font; no generated imitation.
import { readFile, writeFile, copyFile, mkdir } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { artwork, backgroundArtwork, svg, paths } from './mushroom-art.mjs';

const assets = new URL('../public/assets/', import.meta.url);
await mkdir(assets, { recursive: true });
await writeFile(new URL('mushroom-hero.svg', assets), svg(artwork.hero));
await writeFile(new URL('mushroom-mini.svg', assets), svg(artwork.mini));
await writeFile(new URL('mushroom-background.svg', assets), svg(backgroundArtwork));
await writeFile(new URL('mushroom-imprint.svg', assets), svg(artwork.mini, true));
await writeFile(new URL('mushroom-favicon.svg', assets), `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" shape-rendering="crispEdges"><path fill="#160d1b" d="M1 0h14v1H1zM0 1h16v14H0zM1 15h14v1H1z"/>${paths(artwork.micro)}</svg>\n`);
await writeFile(new URL('../../linux/mukhomor.svg', import.meta.url), svg(artwork.mini));
// Android launchers keep their platform safe area; notifications use the 24px grid.
for(const [name,art,mono,viewport,transform] of [
 ['ic_mushroom',artwork.mini,false,108,'android:scaleX="2" android:scaleY="2" android:translateX="22" android:translateY="22"'],
 ['ic_mushroom_monochrome',artwork.mini,true,108,'android:scaleX="2" android:scaleY="2" android:translateX="22" android:translateY="22"'],
 ['ic_stat_mushroom',artwork.compact,true,24,''],
]) {
 const shapes=paths(art,mono).replaceAll('<path fill=','<path android:fillColor=').replaceAll(' d=',' android:pathData=');
 await writeFile(new URL(`../../android/res/drawable/${name}.xml`,import.meta.url),`<?xml version="1.0" encoding="utf-8"?>\n<!-- Generated from assets/mushroom-art.json. -->\n<vector xmlns:android="http://schemas.android.com/apk/res/android" android:width="${viewport}dp" android:height="${viewport}dp" android:viewportWidth="${viewport}" android:viewportHeight="${viewport}"><group ${transform}>${shapes}</group></vector>\n`);
}
await copyFile(new URL('../../assets/fonts/Tiny5-Regular.ttf', import.meta.url), new URL('Tiny5-Regular.ttf', assets));
await copyFile(new URL('../../Tiny5-LICENSE.txt', import.meta.url), new URL('Tiny5-LICENSE.txt', assets));
await copyFile(new URL('../../assets/preview.png', import.meta.url), new URL('app-preview.png', assets));
await copyFile(new URL('../../assets/github-social-preview.png', import.meta.url), new URL('social-preview.png', assets));
console.log(`Exported native pixel artwork, font with license and app preview to ${fileURLToPath(assets)}`);
