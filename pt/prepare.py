"""Prepare the pinned Snowflake source with a narrow injectable-network patch.

Only socket creation and infrastructure DNS are changed; transport protocols,
cryptography and certificate validation remain upstream. Never patch a Go cache.
"""
from pathlib import Path
import argparse, json, os, subprocess, tempfile, zipfile

def write_if_changed(path, data):
    """Publish complete files; repeated/concurrent builds never patch each other."""
    path.parent.mkdir(parents=True, exist_ok=True)
    if path.exists() and path.read_bytes() == data:
        return
    fd, temporary = tempfile.mkstemp(prefix=path.name+'.', dir=path.parent)
    try:
        with os.fdopen(fd, 'wb') as stream:
            stream.write(data)
        os.replace(temporary, path)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)

def extract_pinned(info, prefix, destination, transform):
    with zipfile.ZipFile(info['Zip']) as archive:
        for entry in archive.infolist():
            if not entry.filename.startswith(prefix):
                raise RuntimeError('Unexpected source path')
            relative = Path(entry.filename[len(prefix):])
            if relative.is_absolute() or '..' in relative.parts:
                raise RuntimeError('Unsafe source path')
            if entry.is_dir():
                continue
            # Always transform pristine, checksum-verified module bytes. Reading a
            # previously extracted file here would race with another build.
            data = transform(relative.as_posix(), archive.read(entry))
            write_if_changed(destination/relative, data)

p=argparse.ArgumentParser();p.add_argument('--go',default='go');a=p.parse_args()
root=Path(__file__).resolve().parent
out=root.parent/'target'/'pt-sources'/('windows' if os.name=='nt' else 'linux');out.mkdir(parents=True,exist_ok=True)
module='gitlab.torproject.org/tpo/anti-censorship/pluggable-transports/snowflake/v2'
info=json.loads(subprocess.check_output([a.go,'mod','download','-json',module+'@v2.14.1'],cwd=root,text=True))
if info.get('Error'):raise RuntimeError('Cannot obtain pinned Snowflake source')
dest=out/'snowflake';dest.mkdir(exist_ok=True)
hooks={
 'func (sc *SocksClient) ListenPacket(network string, locAddr *net.UDPAddr) (transport.UDPConn, error) {': '\n\tif localNetwork != nil { return localNetwork.ListenUDP(network, locAddr) }',
 'func (sc *SocksClient) ResolveUDPAddr(network string, address string) (*net.UDPAddr, error) {':'\n\tif localNetwork != nil { return localNetwork.ResolveUDPAddr(network, address) }',
 'func NewTransportWrapper(sc *SocksClient, innerNet transport.Net) transport.Net {':'\n\tif localNetwork != nil { return localNetwork }',
}
def patch_snowflake(relative, data):
    if relative != 'common/proxy/client.go':return data
    text=data.decode('utf-8')
    for original,addition in hooks.items():
        if text.count(original)!=1:raise RuntimeError('Pinned Snowflake network hook changed')
        text=text.replace(original,original+addition)
    text+='\n// Mukhomor: set once, before any transport goroutines are started.\nvar localNetwork transport.Net\nfunc SetLocalNetwork(n transport.Net) { localNetwork = n }\n'
    return text.encode('utf-8')
extract_pinned(info, module+'@v2.14.1/', dest, patch_snowflake)
mod=(root/'go.mod').read_text(encoding='utf-8')+'\nreplace '+module+' => ../'+dest.relative_to(root.parent).as_posix()+'\n'
# Keep anet's IPv6 zone caches local instead of writing private Go internals.
# Scoped remote addresses are disallowed by our underlay. Linker checks stay on.
anet='github.com/wlynxg/anet'; version='v0.0.5'
info=json.loads(subprocess.check_output([a.go,'mod','download','-json',anet+'@'+version],cwd=root,text=True))
if info.get('Error'):raise RuntimeError('Cannot obtain pinned Android interface source')
anetdest=out/'anet';anetdest.mkdir(exist_ok=True)
def patch_anet(relative, data):
    if relative=='interface_android.go':
        for directive in (b'//go:linkname zoneCache net.zoneCache',b'//go:linkname zoneCacheX golang.org/x/net/internal/socket.zoneCache'):
            if data.count(directive)!=1:raise RuntimeError('Pinned Android interface hook changed')
            data=data.replace(directive,b'// Mukhomor: private cache; do not alter Go runtime internals.')
    return data
extract_pinned(info, anet+'@'+version+'/', anetdest, patch_anet)
mod+='\nreplace '+anet+' => ../'+anetdest.relative_to(root.parent).as_posix()+'\n'
write_if_changed(out/'pt.mod',mod.encode('utf-8'))
if (root/'go.sum').exists():write_if_changed(out/'pt.sum',(root/'go.sum').read_bytes())
print(out/'pt.mod')
