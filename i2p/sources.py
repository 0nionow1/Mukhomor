"""Package the exact, checksum-pinned I2P source archives and license notices."""
import hashlib
import shutil
from pathlib import Path
import tarfile
import zipfile
import urllib.request
from build import SOURCES
from prepare import ROOT, VERSION, SHA256, URL


def package():
    sources={"i2pd-"+VERSION+".tar.gz":SHA256, **{name:item[0] for name,item in SOURCES.items()}}
    cache=ROOT/'target/i2p-sources'
    cache.mkdir(parents=True, exist_ok=True)
    target=ROOT/'Source/i2p-dependencies.zip'
    target.parent.mkdir(exist_ok=True)
    pin=ROOT/'i2p/dependencies.sha256'
    expected=pin.read_text().split()[0]
    if target.exists() and pin.exists() and hashlib.file_digest(target.open('rb'),'sha256').hexdigest()==pin.read_text().split()[0]:
        return
    urls={"i2pd-"+VERSION+".tar.gz":URL, **{name:item[1] for name,item in SOURCES.items()}}
    for name,digest in sources.items():
        path=cache/name
        if not path.exists():
            pending=path.with_suffix(path.suffix+'.pending')
            try:
                with urllib.request.urlopen(urls[name],timeout=90) as response, pending.open('wb') as output:
                    shutil.copyfileobj(response,output)
                with pending.open('rb') as source:
                    if hashlib.file_digest(source,'sha256').hexdigest()!=digest:
                        raise RuntimeError('I2P source download checksum mismatch: '+name)
                pending.replace(path)
            finally:
                pending.unlink(missing_ok=True)
    notice=["Mukhomor I2P connector — corresponding third-party licenses\n",
            "Pinned sources are included in Source/i2p-dependencies.zip.\n",
            "The private adapter and source patches are in i2p/ and pt/i2p.go.\n"]
    with zipfile.ZipFile(target.with_suffix('.zip.new'),'w',zipfile.ZIP_STORED) as output:
        for name,digest in sources.items():
            path=cache/name
            with path.open('rb') as source:
                if hashlib.file_digest(source,'sha256').hexdigest()!=digest:
                    raise RuntimeError('I2P source checksum mismatch: '+name)
            info=zipfile.ZipInfo(name,(2026,1,1,0,0,0))
            # Match the reviewed archive on Windows and Linux alike.
            info.create_system=0
            output.writestr(info,path.read_bytes())
            notice.append('\n=== '+name+' (SHA-256 '+digest+') ===\n')
            with tarfile.open(path) as archive:
                members=[m for m in archive.getmembers() if m.isfile() and len(Path(m.name).parts)==2 and Path(m.name).name.lower() in ('license','license.txt','license_1_0.txt','license.md','readme')]
                # zlib's copyright and permission grant are in its README.
                for item in members:
                    if Path(item.name).name.lower()=='readme' and not name.startswith('zlib-'): continue
                    notice.append(archive.extractfile(item).read().decode('utf-8')+'\n')
    with target.with_suffix('.zip.new').open('rb') as pending:
        if hashlib.file_digest(pending,'sha256').hexdigest()!=expected:
            raise RuntimeError('I2P source archive differs from reviewed pin')
    target.with_suffix('.zip.new').replace(target)
    (ROOT/'i2p/THIRD_PARTY_NOTICES.txt').write_text(''.join(notice),encoding='utf-8')
    print('Packaged verified I2P source archives')


if __name__=='__main__':
    package()
