import { readFileSync, existsSync } from 'node:fs';
import { fail, requireThat } from './http.mjs';

export const CONTENT_LIMIT = 262144;
export const REPOSITORY = 'https://github.com/0nionow1/Mukhomor';
const versionPattern = /^(0|[1-9]\d{0,5})\.(0|[1-9]\d{0,5})\.(0|[1-9]\d{0,5})(?:-([0-9A-Za-z]+(?:[.-][0-9A-Za-z]+)*))?$/;
const exact = (value, keys) => value && typeof value === 'object' && !Array.isArray(value) && Object.keys(value).sort().join('|') === [...keys].sort().join('|');
const string = (value, max, empty = false) => typeof value === 'string' && (empty || value.trim().length > 0) && value.length <= max && !/[\u0000-\u0008\u000b\u000c\u000e-\u001f]/.test(value);
export function compareVersions(left, right) {
  const a = left.match(versionPattern), b = right.match(versionPattern);
  for (let i = 1; i <= 3; i++) if (+a[i] !== +b[i]) return +a[i] - +b[i];
  if (!a[4] || !b[4]) return a[4] ? -1 : b[4] ? 1 : 0;
  const x = a[4].split('.'), y = b[4].split('.');
  for (let i = 0; i < Math.max(x.length,y.length); i++) {
    if (x[i] === undefined || y[i] === undefined) return x[i] === undefined ? -1 : 1;
    if (x[i] === y[i]) continue;
    const nx=/^\d+$/.test(x[i]), ny=/^\d+$/.test(y[i]);
    return nx && ny ? Number(x[i])-Number(y[i]) : nx !== ny ? (nx ? -1 : 1) : x[i].localeCompare(y[i], 'en');
  }
  return 0;
}
export function recommendedRelease(document) {
  const visible = document.releases.filter(item => item.visible && item.status === 'stable');
  return document.recommended ? visible.find(item => item.version === document.recommended) : visible.sort((a,b) => compareVersions(b.version,a.version))[0];
}
export function validateContent(document, fields, sources) {
  requireThat(exact(document,['texts','releases','recommended']),400,'invalid_content');
  requireThat(Buffer.byteLength(JSON.stringify(document)) <= CONTENT_LIMIT,413,'body_too_large');
  requireThat(exact(document.texts,fields.map(field=>field.key)),400,'invalid_text_fields');
  for (const field of fields) requireThat(string(document.texts[field.key],field.maxLength,field.allowEmpty),400,'invalid_text');
  requireThat(Array.isArray(document.releases) && document.releases.length <= 80,400,'invalid_releases');
  const versions = new Set();
  for (const release of document.releases) {
    requireThat(exact(release,['version','title','changes','status','visible','publishedAt','files']),400,'invalid_release');
    requireThat(typeof release.version === 'string' && release.version.length <= 60 && versionPattern.test(release.version) && !versions.has(release.version),400,'invalid_version');
    versions.add(release.version);
    requireThat(string(release.title,160) && Array.isArray(release.changes) && release.changes.length >= 1 && release.changes.length <= 30 && release.changes.every(change=>string(change,1000)),400,'invalid_release_notes');
    requireThat(['stable','testing'].includes(release.status) && typeof release.visible === 'boolean',400,'invalid_release_status');
    requireThat(release.status !== 'stable' || !release.version.includes('-'),400,'prerelease_cannot_be_stable');
    const source = sources.get(release.version);
    requireThat(source && release.publishedAt === source.publishedAt && JSON.stringify(release.files) === JSON.stringify(source.files),400,'import_release_from_github');
    requireThat(release.status !== 'stable' || !source.prerelease,400,'github_prerelease_cannot_be_stable');
  }
  requireThat(document.recommended === null || typeof document.recommended === 'string' && document.releases.some(item=>item.version===document.recommended && item.visible && item.status==='stable'),400,'invalid_recommended_release');
  return structuredClone(document);
}
export function publicContent(document) {
  return {...document,releases:document.releases.filter(item=>item.visible).sort((a,b)=>compareVersions(b.version,a.version))};
}
export async function importGithubRelease(version, fetcher = fetch) {
  requireThat(typeof version === 'string',400,'invalid_version');
  version=version.replace(/^v/,'');
  requireThat(version.length <= 60 && versionPattern.test(version),400,'invalid_version');
  let data;
  try {
    const response=await fetcher(`https://api.github.com/repos/0nionow1/Mukhomor/releases/tags/v${encodeURIComponent(version)}`,{
      headers:{Accept:'application/vnd.github+json','User-Agent':'Mukhomor-site-admin','X-GitHub-Api-Version':'2022-11-28'},
      signal:AbortSignal.timeout(6000),redirect:'error'
    });
    if (response.status===404) throw fail(404,'github_release_not_found');
    requireThat(response.ok,503,'github_unavailable');
    const chunks=[];let size=0;
    for await (const chunk of response.body) {size+=chunk.length;requireThat(size<=1048576,503,'github_response_too_large');chunks.push(chunk);}
    data=JSON.parse(Buffer.concat(chunks).toString('utf8'));
  } catch(error) {if(error.status) throw error;throw fail(503,'github_unavailable');}
  requireThat(data.tag_name===`v${version}` && data.draft===false && typeof data.prerelease==='boolean' && /^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ$/.test(data.published_at) && Number.isFinite(Date.parse(data.published_at)) && Array.isArray(data.assets),400,'github_release_not_public');
  const prefix=`${REPOSITORY}/releases/download/v${version}/`;
  const files=data.assets.filter(item=>typeof item.name==='string' && /^[A-Za-z0-9._-]{1,160}$/.test(item.name) && !/source|\.sha256$|\.sig$/i.test(item.name) && /\.(zip|exe|msi|apk|deb|rpm|AppImage|tar\.gz)$/i.test(item.name)).slice(0,20).map(item=>{
    requireThat(item.browser_download_url===prefix+item.name && Number.isSafeInteger(item.size) && item.size>0,400,'invalid_github_asset');
    return {name:item.name,url:item.browser_download_url,size:item.size};
  });
  return {version,publishedAt:data.published_at,prerelease:data.prerelease,files};
}
export function loadContentSeed() {
  const deployed=existsSync(new URL('./content-seed.json',import.meta.url));
  const base=new URL(deployed?'./':'../',import.meta.url);
  const file=deployed?'content-seed.json':'content.json';
  const document=JSON.parse(readFileSync(new URL(file,base),'utf8'));
  const fields=JSON.parse(readFileSync(new URL('content-fields.json',base),'utf8'));
  const sources=new Map(document.releases.map(release=>[release.version,{version:release.version,publishedAt:release.publishedAt,files:release.files,prerelease:release.version.includes('-')} ]));
  validateContent(document,fields,sources);
  return {document,fields,sources};
}
