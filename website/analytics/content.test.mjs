import test from 'node:test';
import assert from 'node:assert/strict';
import { randomBytes } from 'node:crypto';
import { once } from 'node:events';
import { openStore } from './store.mjs';
import { createAnalytics } from './server.mjs';
import { hashPassword } from './admin.mjs';
import { loadContentSeed, validateContent, publicContent, recommendedRelease, compareVersions, importGithubRelease } from './content.mjs';
import { renderReleases } from '../scripts/content.mjs';

const seed=loadContentSeed(), password='test-content-password-42',passwordHash=await hashPassword(password),origin='https://stats.example.test';
async function fixture(t) {
  const store=openStore(':memory:');let delivered;
  const config={origin:'https://0nionow1.github.io',hostname:'0nionow1.github.io',secret:randomBytes(32).toString('hex'),proxyKey:randomBytes(32).toString('hex'),dailySessions:100,dailyEvents:1000,
    admin:{origin,username:'owner',passwordHash,telegram:{token:'123456:'+'a'.repeat(35),chatId:'1234567'}}};
  const imported={version:'0.8.0',publishedAt:'2026-10-15T12:00:00Z',files:[],prerelease:false};
  const {server}=createAnalytics(config,{store,sendAdminCode:async data=>{delivered=data.code;},importRelease:async version=>{assert.equal(version,'0.8.0');return imported;}});
  server.listen(0,'127.0.0.1');await once(server,'listening');
  t.after(async()=>{server.closeAllConnections();await new Promise(resolve=>server.close(resolve));store.close();});
  const headers={'X-Analytics-Proxy-Key':config.proxyKey,'X-Analytics-IP':'192.0.2.9','X-Analytics-Proto':'https','X-Analytics-Host':new URL(origin).host,'User-Agent':'content-test','X-Admin-Request':'1',Origin:origin,'Content-Type':'application/json'};
  const call=(path,{body,extra={},method=body===undefined?'GET':'POST'}={})=>fetch(`http://127.0.0.1:${server.address().port}`+path,{method,headers:{...headers,...extra},...(body===undefined?{}:{body:typeof body==='string'?body:JSON.stringify(body)})});
  let auth={};
  const login=async()=>{const first=await call('/admin/api/login',{body:{username:'owner',password}});assert.equal(first.status,202);const pending=first.headers.get('set-cookie').split(';')[0];const second=await call('/admin/api/verify',{body:{code:delivered},extra:{Cookie:pending}});assert.equal(second.status,200);auth={Cookie:second.headers.get('set-cookie').split(';')[0],'X-CSRF-Token':(await second.json()).csrf};return pending;};
  const admin=(path='',body,extra={})=>call('/admin/api/site'+path,{body,extra:{...auth,...extra}});
  const get=async()=>{const response=await admin();assert.equal(response.status,200);return response.json();};
  const published=(extra={},method='GET')=>call('/v1/site',{method,extra:{Origin:config.origin,...extra}});
  return {store,call,login,admin,get,published,config};
}

test('CMS draft is private; password alone cannot read or change site contents',async t=>{
  const f=await fixture(t);assert.equal((await f.published()).status,503);
  assert.equal((await f.admin()).status,401);
  const pending=await f.login();assert.equal((await f.admin('',undefined,{Cookie:pending})).status,401);
  assert.equal((await f.admin('/save',{revision:1,document:seed.document},{'X-CSRF-Token':'wrong'})).status,403);
  assert.equal((await f.admin('/publish',{revision:1},{Origin:'https://evil.example'})).status,403);
  assert.equal((await f.admin('/publish',{revision:1},{'Sec-Fetch-Site':'cross-site'})).status,403);
  assert.equal((await f.published()).status,503);
});
test('saving and explicit publishing are atomic; public API exposes only the published visible releases',async t=>{
  const f=await fixture(t);await f.login();const state=await f.get();
  const document=structuredClone(state.document);document.releases[1].visible=false;document.texts[seed.fields[0].key]='Публичный текст';
  const saved=await f.admin('/save',{revision:state.revision,document});assert.equal(saved.status,200);const next=await saved.json();
  assert.equal((await f.published()).status,503);
  assert.equal((await f.admin('/publish',{revision:next.revision})).status,200);
  const publicResponse=await f.published();assert.equal(publicResponse.status,200);const data=await publicResponse.json();
  assert.deepEqual(data.document.releases.map(item=>item.version),document.releases.filter(item=>item.visible).map(item=>item.version));
  assert.equal(data.document.releases.some(item=>item.version===document.releases[1].version),false);
  assert.equal(data.document.texts[seed.fields[0].key],'Публичный текст');
  assert.equal(data.fields,undefined);assert.equal(data.audit,undefined);assert.equal(publicResponse.headers.get('access-control-allow-credentials'),null);
  assert.equal((await f.published({'If-None-Match':publicResponse.headers.get('etag')})).status,304);
  assert.equal((await f.published({Origin:'https://evil.example'})).status,403);
  assert.equal((await f.published({},'HEAD')).status,200);
  assert.equal((await f.published({},'POST')).status,405);
  assert.equal((await f.call('/v1/site?draft=1')).status,403);
});
test('concurrent edits reject stale revisions and restoration only creates a draft',async t=>{
  const f=await fixture(t);await f.login();const state=await f.get();
  const responses=await Promise.all([f.admin('/save',{revision:state.revision,document:state.document}),f.admin('/save',{revision:state.revision,document:state.document})]);
  assert.deepEqual(responses.map(item=>item.status).sort(),[200,409]);
  const second=await f.get();await f.admin('/publish',{revision:second.revision});
  second.document.texts[seed.fields[0].key]='Черновик';await f.admin('/save',{revision:second.revision,document:second.document});const third=await f.get();
  const restored=await f.admin('/restore',{revision:third.revision,fromRevision:second.revision});assert.equal(restored.status,200);const result=await restored.json();
  assert.equal(result.document.texts[seed.fields[0].key],seed.document.texts[seed.fields[0].key]);assert.equal(result.publishedRevision,second.revision);assert.equal(result.revision,third.revision+1);
});
test('release import is authenticated, bounded and separate from publication; status and recommendation are validated',async t=>{
  const f=await fixture(t);await f.login();const imported=await f.admin('/import',{version:'0.8.0'});assert.equal(imported.status,200);const item=await imported.json();
  assert.equal(item.status,'testing');const state=await f.get();assert.equal(state.document.releases.some(item=>item.version==='0.8.0'),false);
  delete item.prerelease;state.document.releases.push(item);
  state.document.recommended='0.8.0';assert.equal((await f.admin('/save',{revision:state.revision,document:state.document})).status,400);
  state.document.recommended=null;assert.equal((await f.admin('/save',{revision:state.revision,document:state.document})).status,200);
  const updated=await f.get();updated.document.releases.at(-1).files=[{name:'evil.exe',url:'https://evil.example/evil.exe',size:1}];
  assert.equal((await f.admin('/save',{revision:updated.revision,document:updated.document})).status,400);
});
test('oversized CMS bodies and unknown keys are rejected without widening login payload limits',async t=>{
  const f=await fixture(t);await f.login();const state=await f.get();
  assert.equal((await f.admin('/save',{revision:state.revision,document:{...state.document,html:'<script>'}})).status,400);
  assert.equal((await f.admin('/save','{"padding":"'+'x'.repeat(270000)+'"}')).status,413);
  assert.equal((await f.call('/admin/api/login',{body:{username:'owner',password:'x'.repeat(5000)}})).status,413);
  assert.equal((await f.get()).revision,state.revision);
});
test('restored and initialized store preserves edits; history and import budgets are bounded',()=>{
  const store=openStore(':memory:');try {
    store.initContent(seed.document,seed.sources);const document=structuredClone(seed.document);document.texts[seed.fields[0].key]='Сохранённый текст';
    let revision=store.saveContent(document,1,Date.now());store.initContent(seed.document,seed.sources);assert.equal(store.getContent().document.texts[seed.fields[0].key],'Сохранённый текст');
    for(let i=0;i<13;i++){store.publishContent(revision,Date.now()+i);revision=store.saveContent(document,revision,Date.now()+i);}
    assert.equal(store.getContent().history.length,10);
    for(let i=0;i<10;i++)assert.equal(store.reserveContentWrite(1000,true),0);assert.ok(store.reserveContentWrite(1000,true)>0);
  } finally {store.close();}
});
test('catalog chooses latest stable semver; testing downloads remain available and HTML is escaped',()=>{
  const document=structuredClone(seed.document);document.releases.push({...document.releases[0],version:'0.10.0',status:'testing'});
  assert.equal(recommendedRelease(document).version,'0.7.2');document.releases.at(-1).status='stable';assert.equal(recommendedRelease(document).version,'0.10.0');
  document.recommended='0.5.0';assert.equal(recommendedRelease(document).version,'0.5.0');
  assert.ok(compareVersions('0.10.0','0.9.0')>0);assert.ok(compareVersions('1.0.0-beta.10','1.0.0-beta.2')>0);
  document.releases[0].title='<img src=x onerror=alert(1)>';const html=renderReleases(document,true);assert.ok(!html.includes('<img'));assert.ok(html.includes('&lt;img'));assert.ok(html.includes('data-analytics="download"'));
  document.releases.find(item=>item.version==='0.5.0').visible=false;assert.equal(publicContent(document).releases.some(item=>item.version==='0.5.0'),false);
});
test('static release catalog keeps downloads available without the content API and shows newest first',()=>{
  const document=structuredClone(seed.document);
  document.releases.push({...document.releases[0],version:'0.10.0',status:'testing'});
  const html=renderReleases(document);
  assert.ok(html.indexOf('id="v0.10.0"') < html.indexOf('id="v0.7.2"'));
  for(const release of seed.document.releases.filter(item=>item.visible))for(const file of release.files)assert.ok(html.includes(`href="${file.url}"`));
  assert.ok(!html.includes('aria-disabled="true"'));
});
test('GitHub importer fixes upstream host and rejects draft, redirected, oversized and foreign files',async()=>{
  const data={tag_name:'v0.8.0',published_at:'2026-10-15T12:00:00Z',draft:false,prerelease:true,assets:[{name:'Mukhomor-0.8.0-android.apk',browser_download_url:'https://github.com/0nionow1/Mukhomor/releases/download/v0.8.0/Mukhomor-0.8.0-android.apk',size:100}]};
  const mock=async(url,options)=>{assert.equal(url,'https://api.github.com/repos/0nionow1/Mukhomor/releases/tags/v0.8.0');assert.equal(options.redirect,'error');return Response.json(data);};
  const imported=await importGithubRelease('v0.8.0',mock);assert.equal(imported.files.length,1);assert.equal(imported.prerelease,true);
  await assert.rejects(importGithubRelease('https://evil.example',mock));
  await assert.rejects(importGithubRelease('0.8.0',async()=>Response.json({...data,draft:true})));
  await assert.rejects(importGithubRelease('0.8.0',async()=>Response.json({...data,assets:[{...data.assets[0],browser_download_url:'https://evil.example/file'}]})));
  await assert.rejects(importGithubRelease('0.8.0',async()=>new Response('x'.repeat(1048577))));
  await assert.rejects(importGithubRelease('0.8.0',async()=>new Response('',{status:302})));
});
