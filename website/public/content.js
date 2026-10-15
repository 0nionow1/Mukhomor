(() => {
  'use strict';
  const config=window.MukhomorSiteContent, snapshot=window.MukhomorSnapshot;
  if(!config || !snapshot)return;
  const status=document.getElementById('catalog-status');
  const make=(tag,text,className)=>{const node=document.createElement(tag);if(text!==undefined)node.textContent=text;if(className)node.className=className;return node;};
  const date=value=>new Intl.DateTimeFormat('ru-RU',{day:'numeric',month:'long',year:'numeric',timeZone:'Europe/Moscow'}).format(new Date(value));
  const compare=(a,b)=>{const x=a.version.split(/[.-]/),y=b.version.split(/[.-]/);for(let i=0;i<3;i++)if(+x[i]!==+y[i])return +y[i]-+x[i];return Date.parse(b.publishedAt)-Date.parse(a.publishedAt);};
  function apply(documentData, fresh) {
    window.MukhomorContent=documentData;
    for(const node of document.querySelectorAll('[data-cms]'))if(typeof documentData.texts[node.dataset.cms]==='string')node.textContent=documentData.texts[node.dataset.cms];
    const visible=documentData.releases.filter(item=>item.visible).sort(compare);
    const recommended=visible.find(item=>item.status==='stable' && (documentData.recommended===null || item.version===documentData.recommended));
    const catalog=document.getElementById('release-catalog');
    if(catalog) {
      const fragment=document.createDocumentFragment();
      const ordered=visible;
      for(const release of ordered) {
        const card=make('article',undefined,'download-release'+(release===recommended?' recommended-release':''));card.id='v'+release.version;
        const meta=make('div',undefined,'download-release-meta'),badges=make('div',undefined,'release-badges');
        badges.append(make('span',release.status==='stable'?'Stable':'В проверке','status '+(release.status==='stable'?'available-status':'testing-status')));
        if(release===recommended)badges.append(make('span','Рекомендуемая','recommended-label'));
        const time=make('time',date(release.publishedAt));time.dateTime=release.publishedAt;meta.append(badges,make('h2',release.version),time);
        const body=make('div',undefined,'download-release-body'),list=make('ul');release.changes.forEach(text=>list.append(make('li',text)));
        body.append(make('h3',release.title),list);
        if(release.status==='testing')body.append(make('p','Тестовая версия. Возможны ошибки.','testing-note'));
        const files=make('div',undefined,'release-files');
        for(const file of release.files) {
          const link=make('a',undefined,'button button-primary release-file');
          const platform=/windows/i.test(file.name)?'для Windows':/android|\.apk$/i.test(file.name)?'для Android':/linux|\.deb$|\.rpm$|\.AppImage$/i.test(file.name)?'для Linux':'файл';
          const expected=`https://github.com/0nionow1/Mukhomor/releases/download/v${release.version}/`;
          if(file.url===expected+file.name && /^[A-Za-z0-9._-]+$/.test(file.name)){link.href=file.url;link.dataset.analytics='download';}else link.setAttribute('aria-disabled','true');
          link.append(make('span','Скачать '+platform),make('small',(file.size/1048576).toFixed(1)+' МБ'));files.append(link);
        }
        const source=make('a','Файлы и SHA256 на GitHub ↗','text-link release-source');source.href='https://github.com/0nionow1/Mukhomor/releases/tag/v'+encodeURIComponent(release.version);source.target='_blank';source.rel='noopener noreferrer';
        body.append(files,source);card.append(meta,body);fragment.append(card);
      }
      if(!ordered.length)fragment.append(make('p','Доступных версий пока нет.'));
      catalog.replaceChildren(fragment);
    }
    if(document.body.classList.contains('changelog-page')) {
      const history=document.getElementById('published-history');
      if(history) {
        history.replaceChildren();
        for(const release of visible) {
          const article=make('article',undefined,'release-entry'),meta=make('div',undefined,'release-meta'),notes=make('div',undefined,'release-notes'),list=make('ul');article.id='v'+release.version;
          const heading=make('h2'),anchor=make('a',release.version),time=make('time',date(release.publishedAt));anchor.href='#v'+release.version;heading.id='v'+release.version+'-title';heading.append(anchor);article.setAttribute('aria-labelledby',heading.id);time.dateTime=release.publishedAt;
          meta.append(make('span',release.status==='stable'?'Stable':'В проверке','status '+(release.status==='stable'?'available-status':'development-status')),heading,time);
          const source=make('a','Релиз на GitHub ↗','text-link release-source');source.href='https://github.com/0nionow1/Mukhomor/releases/tag/v'+encodeURIComponent(release.version);source.target='_blank';source.rel='noopener noreferrer';
          release.changes.forEach(change=>list.append(make('li',change)));notes.append(make('h3',release.title),list,source);article.append(meta,notes);history.append(article);
        }
      }
    }
    const schema=document.querySelector('script[type="application/ld+json"]');
    if(schema) {
      try {
        const value=JSON.parse(schema.textContent);
        for(const entity of value['@graph']||[]) {
          if(entity['@type']==='SoftwareApplication') {if(visible[0])entity.softwareVersion=visible[0].version;else delete entity.softwareVersion;}
          if(entity['@type']==='FAQPage') for(const [i,entry] of entity.mainEntity.entries()) {const section=document.querySelectorAll('.faq-answer')[i];if(section){entry.name=section.querySelector('h2').textContent;entry.acceptedAnswer.text=section.querySelector('p').textContent;}}
        }
        schema.textContent=JSON.stringify(value).replace(/</g,'\\u003c');
      } catch { /* Keep the static structured data if a snapshot is unavailable. */ }
    }
    document.dispatchEvent(new CustomEvent('mukhomor-content',{detail:documentData}));
  }
  apply(snapshot,false);
  if(['localhost','127.0.0.1','[::1]'].includes(location.hostname)) {apply(snapshot,true);if(status)status.textContent='Предпросмотр каталога';return;}
  if(!config.enabled) {if(status)status.textContent='Актуальные файлы доступны в выпусках на GitHub.';return;}
  (async()=>{
    try {
      const endpoint=new URL('/v1/site',config.endpoint);
      if(endpoint.protocol!=='https:')throw new Error('HTTPS required');
      const response=await fetch(endpoint,{credentials:'omit',redirect:'error',signal:AbortSignal.timeout(7000),referrerPolicy:'no-referrer'});
      if(!response.ok)throw new Error('Unavailable');
      const chunks=[];let size=0;const reader=response.body.getReader();
      for(;;){const {value,done}=await reader.read();if(done)break;size+=value.length;if(size>300000){await reader.cancel();throw new Error('Oversized');}chunks.push(value);}
      const buffer=new Uint8Array(size);let offset=0;for(const chunk of chunks){buffer.set(chunk,offset);offset+=chunk.length;}
      const data=JSON.parse(new TextDecoder().decode(buffer));
      if(!data.document?.texts || !Array.isArray(data.document.releases))throw new Error('Invalid catalog');
      apply(data.document,true);
      if(status)status.textContent='Каталог обновлён '+date(data.publishedAt)+'.';
    } catch {
      if(status)status.textContent='Показан каталог на момент обновления сайта. Скачать файлы можно ниже; все выпуски доступны на GitHub.';
    }
  })();
})();
