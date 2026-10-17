"""Collect pinned public Go module sources and their license notices."""
import argparse, hashlib, json, os, subprocess, zipfile
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('--go',default='go');p.add_argument('--source-only',action='store_true');p.add_argument('--offline',action='store_true');a=p.parse_args()
root=Path(__file__).resolve().parent
env=os.environ.copy()
if a.offline:env['GOPROXY']='off'
result=subprocess.check_output([a.go,'mod','download','-json','all'],cwd=root,env=env,text=True)
decoder=json.JSONDecoder();modules=[]
while result.strip():
    result=result.lstrip();item,end=decoder.raw_decode(result);result=result[end:]
    if item.get('Error') or not item.get('Zip') or not item.get('Sum'):raise RuntimeError('Pinned module unavailable')
    modules.append(item)
dest=root.parent/'Source'/'pt-dependencies.zip';dest.parent.mkdir(exist_ok=True)
pending=dest.with_suffix('.pending')
notices=['Mukhomor Tor bridge adapter: pinned Lyrebird 0.9.0 and Snowflake 2.14.1.\nExact versions and integrity: pt/go.mod and pt/go.sum.\nCorresponding source: Source/pt-dependencies.zip.\nSnowflake socket integration: pt/prepare.py (protocols and cryptography unchanged).\n']
with zipfile.ZipFile(pending,'w',zipfile.ZIP_STORED) as out:
    for item in sorted(modules,key=lambda m:m['Path']):
        name=item['Path']+'/@v/'+item['Version']
        for field,suffix in [('Zip','.zip'),('GoMod','.mod'),('Info','.info')]:
            data=Path(item[field]).read_bytes();entry=zipfile.ZipInfo(name+suffix,(1980,1,1,0,0,0));entry.create_system=0;out.writestr(entry,data)
        if a.source_only:continue
        notices.append('\n'+'='*72+'\n'+item['Path']+' '+item['Version']+'\n'+item['Sum'])
        with zipfile.ZipFile(item['Zip']) as source:
            licenses=[n for n in source.namelist() if Path(n).name.lower().startswith(('license','copying','notice','copyright')) and len(n.split('/'))<=len(item['Path'].split('/'))+2]
            for name in licenses:
                if not name.endswith('/'):notices.append(source.read(name).decode('utf-8',errors='replace'))
digest=hashlib.sha256(pending.read_bytes()).hexdigest()
pin=root/'dependencies.sha256'
if a.source_only:
    if pin.read_text().split()[0]!=digest:raise RuntimeError('Bridge source checksum differs from reviewed pin')
else:
    pin.write_text(digest+'  pt-dependencies.zip\n',encoding='utf-8')
    (root/'THIRD_PARTY_NOTICES.txt').write_text('\n'.join(notices)+'\n',encoding='utf-8')
pending.replace(dest)
print('Verified bridge sources:',len(modules),'modules')
