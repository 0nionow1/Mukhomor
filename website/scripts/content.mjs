import { readFile, writeFile } from 'node:fs/promises';
import { pathToFileURL } from 'node:url';
import assert from 'node:assert/strict';
import { validateContent, publicContent, recommendedRelease, compareVersions } from '../analytics/content.mjs';

const root=new URL('../',import.meta.url);
export const escape=value=>String(value).replace(/[&<>"']/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
export function renderReleases(document, downloads=true) {
  const recommended=recommendedRelease(document);
  const date=value=>new Intl.DateTimeFormat('ru-RU',{day:'numeric',month:'long',year:'numeric',timeZone:'Europe/Moscow'}).format(new Date(value));
  const ordered=document.releases.filter(item=>item.visible).sort((a,b)=>compareVersions(b.version,a.version));
  return ordered.map(release=>`<article class="download-release${release.version===recommended?.version?' recommended-release':''}" id="v${escape(release.version)}"><div class="download-release-meta"><div class="release-badges"><span class="status ${release.status==='stable'?'available-status':'testing-status'}">${release.status==='stable'?'Stable':'В проверке'}</span>${release.version===recommended?.version?'<span class="recommended-label">Рекомендуемая</span>':''}</div><h2>${escape(release.version)}</h2><time datetime="${escape(release.publishedAt)}">${date(release.publishedAt)}</time></div><div class="download-release-body"><h3>${escape(release.title)}</h3><ul>${release.changes.map(change=>`<li>${escape(change)}</li>`).join('')}</ul>${release.status==='testing'?'<p class="testing-note">Тестовая версия. Возможны ошибки.</p>':''}<div class="release-files">${release.files.map(file=>`<a class="button button-primary release-file" ${downloads?`href="${escape(file.url)}" data-analytics="download"`:'aria-disabled="true"'}><span>Скачать ${/windows/i.test(file.name)?'для Windows':/android|\.apk$/i.test(file.name)?'для Android':/linux|\.deb$|\.rpm$|\.AppImage$/i.test(file.name)?'для Linux':'файл'}</span><small>${(file.size/1048576).toFixed(1)} МБ</small></a>`).join('')}</div><a class="text-link release-source" href="https://github.com/Mukhomo/Mukhomor/releases/tag/v${escape(release.version)}" target="_blank" rel="noopener noreferrer">Файлы и SHA256 на GitHub ↗</a></div></article>`).join('\n');
}
export async function syncContent(checkOnly=false) {
  const document=JSON.parse(await readFile(new URL('content.json',root),'utf8'));
  const fields=JSON.parse(await readFile(new URL('content-fields.json',root),'utf8'));
  const sources=new Map(document.releases.map(release=>[release.version,{...release,prerelease:release.version.includes('-')}]));
  validateContent(document,fields,sources);
  const write=async(name,value)=>{const file=new URL('public/'+name,root);if(checkOnly)assert.equal(await readFile(file,'utf8'),value,`Stale content: ${name}; run node website/scripts/content.mjs`);else await writeFile(file,value);};
  for(const page of ['index','docs','privacy','policy','releases']) {
    let html=await readFile(new URL('public/'+page+'.html',root),'utf8');
    html=html.replace(/(<span data-cms="([^"]+)">)[\s\S]*?(<\/span>)/g,(_,open,key,close)=>{assert(Object.hasOwn(document.texts,key),'Unknown content field '+key);return open+escape(document.texts[key])+close;});
    if(page==='releases')html=html.replace(/<!-- releases:start -->[\s\S]*?<!-- releases:end -->/,`<!-- releases:start -->\n${renderReleases(document)}\n<!-- releases:end -->`);
    await write(page+'.html',html);
  }
  await write('content-snapshot.js','window.MukhomorSnapshot = '+JSON.stringify(publicContent(document)).replace(/</g,'\\u003c')+';\n');
  return document;
}
if(process.argv[1] && import.meta.url===pathToFileURL(process.argv[1]).href) {
  const snapshot=process.argv.indexOf('--snapshot');
  if(snapshot!==-1) {
    const document=JSON.parse(await readFile(process.argv[snapshot+1],'utf8'));
    const fields=JSON.parse(await readFile(new URL('content-fields.json',root),'utf8'));
    validateContent(document,fields,new Map(document.releases.map(item=>[item.version,{...item,prerelease:item.version.includes('-')}])));
    await writeFile(new URL('content.json',root),JSON.stringify(document,null,2)+'\n');
  }
  await syncContent(process.argv.includes('--check'));
  console.log('Content snapshot and release page verified.');
}
