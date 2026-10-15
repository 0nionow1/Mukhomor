import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {runInNewContext} from 'node:vm';
import {randomUUID} from 'node:crypto';

const source=readFileSync(new URL('../public/analytics.js',import.meta.url),'utf8');
const KEY='mukhomor.analytics.',PERIOD=180*86400000,NOW=Date.parse('2026-10-15T12:00:00Z');
const choice=(accepted,at=NOW,version='2026-10-15.1')=>({version,accepted,at,expires:at+PERIOD});
const drain=async()=>{for(let i=0;i<30;i++)await Promise.resolve();};
function browser({enabled=true,stored={},dnt=false,gpc=false,storageBlocked=false,holdSession=false}={}) {
  const data=new Map(Object.entries(stored).map(([key,value])=>[key,JSON.stringify(value)]));
  const calls=[],scripts=[],challenges=[],aborted=[];let clock=NOW,resolveSession;
  class Node {
    constructor(){this.children=[];this.listeners=new Map();this.hidden=false;this.dataset={};}
    addEventListener(name,callback){const list=this.listeners.get(name)||[];list.push(callback);this.listeners.set(name,list);}
    dispatch(name,event={}){for(const callback of this.listeners.get(name)||[])callback(event);}
    append(...children){for(const child of children){child.parent=this;this.children.push(child);if(child.src){scripts.push(child.src);queueMicrotask(()=>{window.turnstile=turnstile;child.onload();});}}}
    remove(){if(this.parent)this.parent.children=this.parent.children.filter(child=>child!==this);}
    setAttribute(name,value){this[name]=value;}
  }
  const body=new Node(),head=new Node(),document=new Node(),window=new Node(),preference=new Node(),settings=new Node(),revokes=[new Node(),new Node()];
  const turnstile={render:(_host,options)=>{challenges.push(options);return challenges.length;},remove:()=>{}};
  Object.assign(document,{body,head,hidden:false,createElement:()=>new Node(),querySelector:selector=>selector==='#analytics-preference-status'?preference:body.children.find(node=>'.'+node.className===selector),querySelectorAll:selector=>selector==='[data-analytics-revoke]'?revokes:selector==='[data-analytics-settings]'?[settings]:[]});
  window.MukhomorAnalytics={enabled,siteUrl:'https://site.example/Mukhomor/',endpoint:'https://stats.example',turnstileSiteKey:'public-test-key'};
  const navigator={doNotTrack:dnt?'1':'0',globalPrivacyControl:gpc};
  const fetch=async(url,options)=>{
    const call={path:new URL(url).pathname,body:JSON.parse(options.body),signal:options.signal};calls.push(call);
    options.signal.addEventListener('abort',()=>aborted.push(call.path));
    if(call.path==='/v1/session') {
      if(holdSession)await new Promise(resolve=>{resolveSession=resolve;});
      return {ok:true,status:200,json:async()=>({token:'session-test',expires:clock+1800000,visitorToken:'visitor-test',visitorExpires:clock+30*86400000})};
    }
    return {ok:true,status:204};
  };
  runInNewContext(source,{window,document,navigator,location:{origin:'https://site.example',pathname:'/Mukhomor/'},URL,AbortController,AbortSignal,crypto:{randomUUID},fetch,
    Date:class extends Date {static now(){return clock;}},Number,JSON,
    localStorage:{getItem:key=>{if(storageBlocked)throw Error('denied');return data.get(key)??null;},setItem:(key,value)=>{if(storageBlocked)throw Error('denied');data.set(key,value);},removeItem:key=>{if(storageBlocked)throw Error('denied');data.delete(key);}},setTimeout:()=>1,clearTimeout:()=>{}});
  const panel=()=>body.children.find(node=>node.className==='analytics-consent');
  return {data,calls,scripts,challenges,aborted,preference,settings,revokes,document,window,navigator,panel,
    choose(value){assert(panel(),'choice panel is visible');panel().dispatch('click',{target:{closest:()=>({dataset:{choice:value}})}});},
    async approve(){challenges.at(-1).callback('test-challenge');await drain();},
    async click(){document.dispatch('click',{isTrusted:true,type:'click',target:{closest:()=>({href:'https://github.com/0nionow1/Mukhomor',dataset:{}})}});await drain();},
    advance(ms){clock+=ms;},completeSession(){resolveSession();},
    storage(value){if(value===null)data.clear();else data.set(KEY+'consent',JSON.stringify(value));window.dispatch('storage',{key:value===null?null:KEY+'consent'});}
  };
}
test('visitor makes no analytics or Turnstile requests before an affirmative choice; closing is not consent',async()=>{
  const b=browser();await b.click();assert.equal(b.calls.length,0);assert.equal(b.scripts.length,0);assert.equal(b.data.size,0);
  const buttons=[...b.panel().innerHTML.matchAll(/class="([^"]+)" data-choice="(?:no|yes)"/g)].map(match=>match[1]);assert.equal(buttons.length,2);assert.equal(buttons[0],buttons[1]);
  b.panel().children.at(-1).dispatch('click');await b.click();assert.equal(b.calls.length,0);assert.equal(b.data.size,0);
  b.settings.dispatch('click');b.choose('no');await b.click();assert.equal(b.calls.length,0);assert.equal(b.scripts.length,0);
  const saved=JSON.parse(b.data.get(KEY+'consent'));assert.equal(saved.accepted,false);assert.equal(saved.expires-saved.at,PERIOD);
});
test('consent precedes challenge, session and events; withdrawal clears identifiers and aborts further tracking',async()=>{
  const b=browser();b.choose('yes');await drain();assert.equal(b.scripts.length,1);assert.equal(b.calls.length,0);await b.approve();
  assert.deepEqual(b.calls.map(call=>call.path),['/v1/session','/v1/event']);assert(b.data.has(KEY+'visitor'));assert(b.revokes.every(button=>!button.hidden));
  b.revokes[0].dispatch('click');await b.click();assert.equal(b.calls.length,2);assert(!b.data.has(KEY+'session'));assert(!b.data.has(KEY+'visitor'));assert.equal(JSON.parse(b.data.get(KEY+'consent')).accepted,false);
  b.settings.dispatch('click');b.choose('yes');await drain();assert.equal(b.challenges.length,2);
});
test('late challenge and response cannot restore identifiers after withdrawal',async()=>{
  const b=browser({holdSession:true});b.choose('yes');await drain();b.challenges[0].callback('test-challenge');await drain();assert.equal(b.calls.length,1);
  b.revokes[0].dispatch('click');assert.deepEqual(b.aborted,['/v1/session']);b.completeSession();await drain();assert.equal(b.calls.length,1);assert(!b.data.has(KEY+'visitor'));assert(!b.data.has(KEY+'session'));
  const c=browser();c.choose('yes');await drain();c.revokes[0].dispatch('click');c.challenges[0].callback('late-token');await drain();assert.equal(c.calls.length,0);
});
test('reject, cleared storage and changed consent in another tab stop pending collection without writing another choice',async()=>{
  for(const next of [choice(false),null]) {
    const b=browser();b.choose('yes');await drain();await b.approve();b.storage(next);await b.click();assert.equal(b.calls.length,2);assert(!b.data.has(KEY+'visitor'));assert(!b.data.has(KEY+'session'));
    if(next===null)assert(!b.data.has(KEY+'consent'));else assert.equal(JSON.parse(b.data.get(KEY+'consent')).accepted,false);
  }
});
test('expired, malformed, older-version and legacy consent never authorise analytics',async()=>{
  for(const saved of [true,choice(true,NOW-PERIOD),choice(true,NOW,'old'),{accepted:true},choice(true,NOW+1000)]) {
    const b=browser({stored:{[KEY+'consent']:saved,[KEY+'visitor']:{token:'old'}}});await b.click();assert(b.panel());assert.equal(b.calls.length,0);assert.equal(b.scripts.length,0);assert(!b.data.has(KEY+'visitor'));
  }
  const b=browser();b.choose('yes');await drain();await b.approve();b.advance(PERIOD+1);await b.click();assert.equal(b.calls.length,2);b.document.dispatch('visibilitychange');assert(b.panel());assert(!b.data.has(KEY+'visitor'));
});
test('disabled analytics, DNT and GPC never load the challenge, even with previous consent',async()=>{
  for(const options of [{enabled:false},{dnt:true},{gpc:true}]) {
    const b=browser({...options,stored:{[KEY+'consent']:choice(true),[KEY+'visitor']:{token:'old'}}});b.settings.dispatch('click');await drain();assert.equal(b.scripts.length,0);assert.equal(b.calls.length,0);assert(!b.data.has(KEY+'visitor'));assert(!b.panel().innerHTML.includes('data-choice="yes"'));
  }
});
test('unavailable storage does not force consent or prevent withdrawal in the current page',async()=>{
  const b=browser({storageBlocked:true});assert(b.panel());b.choose('yes');await drain();await b.approve();assert.equal(b.calls.length,2);b.revokes[0].dispatch('click');await b.click();assert.equal(b.calls.length,2);assert.equal(b.data.size,0);
});
