"""Build the owned i2pd child from pinned sources (Linux / MinGW / Android NDK)."""
import argparse
import hashlib
import json
import os
import platform as host_platform
from pathlib import Path
import shutil
import subprocess
import tarfile
import urllib.request
import zipfile
from prepare import ROOT, prepare, atomic_write

ARTIFACT_INPUTS = ('i2p/main.cpp', 'i2p/CMakeLists.txt', 'i2p/build.py',
                   'i2p/prepare.py', 'i2p/dependencies.sha256')


def export_artifact(binary, platform, directory):
    """Keep same-run CI artifacts tied to their exact reviewed build inputs."""
    destination = directory / platform
    destination.mkdir(parents=True, exist_ok=True)
    data = binary.read_bytes()
    atomic_write(destination / binary.name, data)
    manifest = {
        'schema': 1, 'platform': platform, 'name': binary.name,
        'size': len(data), 'sha256': hashlib.sha256(data).hexdigest(),
        'inputs': {name: hashlib.sha256((ROOT / name).read_text(encoding='utf-8-sig').replace('\r\n', '\n').encode()).hexdigest()
                   for name in ARTIFACT_INPUTS},
    }
    atomic_write(destination / 'manifest.json', (json.dumps(manifest, indent=2) + '\n').encode())

SOURCES = {
    'boost_1_90_0.tar.bz2': ('49551aff3b22cbc5c5a9ed3dbc92f0e23ea50a0f7325b0d198b705e8ee3fc305', 'https://archives.boost.io/release/1.90.0/source/boost_1_90_0.tar.bz2'),
    'openssl-3.5.9.tar.gz': ('603f5602e2eef00d77fbd429d34dcd5822bb301757a1bc9cdb24c670f1eb859a', 'https://github.com/openssl/openssl/releases/download/openssl-3.5.9/openssl-3.5.9.tar.gz'),
    'zlib-1.3.2.tar.gz': ('bb329a0a2cd0274d05519d61c667c062e06990d72e125ee2dfa8de64f0119d16', 'https://github.com/madler/zlib/releases/download/v1.3.2/zlib-1.3.2.tar.gz'),
}


def source(cache, name):
    digest, url = SOURCES[name]
    archive = cache / name
    if not archive.exists():
        existing = ROOT / 'target/i2p-sources' / name
        if existing.exists() and existing!=archive:
            shutil.copyfile(existing,archive)
        bundle=ROOT/'Source/i2p-dependencies.zip'
        pin=ROOT/'i2p/dependencies.sha256'
        if not archive.exists() and bundle.exists() and pin.exists():
            if hashlib.sha256(bundle.read_bytes()).hexdigest()!=pin.read_text().split()[0]:
                raise RuntimeError('I2P corresponding source checksum mismatch')
            with zipfile.ZipFile(bundle) as package:
                atomic_write(archive,package.read(name))
        existing = ROOT / 'target/i2p-sources' / name
        if not existing.exists(): existing = ROOT / 'target/p2p-research' / name
        if archive.exists():
            pass
        elif existing.exists() and hashlib.sha256(existing.read_bytes()).hexdigest() == digest:
            shutil.copyfile(existing, archive)
        else:
            with urllib.request.urlopen(url, timeout=60) as remote, archive.open('wb') as out:
                shutil.copyfileobj(remote, out)
    if hashlib.sha256(archive.read_bytes()).hexdigest() != digest:
        raise RuntimeError('Source checksum mismatch: ' + name)
    with tarfile.open(archive) as tar:
        top = Path(tar.next().name).parts[0]
        if top in ('.','..') or Path(top).is_absolute(): raise RuntimeError('Unsafe source archive root')
        dest = cache / top
        stamp=dest / '.mukhomor-source-complete'
        if not stamp.exists() or stamp.read_text()!=digest:
            for item in tar.getmembers():
                path = Path(item.name)
                if path.is_absolute() or '..' in path.parts or path.parts[0] != top:
                    raise RuntimeError('Unsafe source archive entry')
                if item.isfile():
                    if name.startswith('boost_') and not (path.as_posix().startswith(top+'/boost/') or path.as_posix().startswith(top+'/libs/program_options/') or path.as_posix()==top+'/LICENSE_1_0.txt'):
                        continue
                    target = cache / path
                    target.parent.mkdir(parents=True, exist_ok=True)
                    raw = tar.extractfile(item).read()
                    if not target.exists() or target.read_bytes() != raw:
                        atomic_write(target, raw)
                    target.chmod(item.mode & 0o777)
            (dest / '.mukhomor-source-complete').write_text(digest)
    return dest


def build(platform, ndk=None, jobs=4):
    if os.name == 'nt':
        raise RuntimeError('Run i2p/build.py inside Linux or WSL')
    # Linux metadata/compilation is much faster on its native filesystem in WSL.
    cache = Path(os.environ.get('MUKHOMOR_I2P_BUILD_CACHE', str(Path.home()/'.cache/mukhomor-i2p-build') if str(ROOT).startswith('/mnt/') else str(ROOT/'target/i2p-sources')))
    cache.mkdir(parents=True, exist_ok=True)
    import fcntl
    with (cache/'.sources.lock').open('w') as lock:
        fcntl.flock(lock,fcntl.LOCK_EX)
        router = prepare(cache)
        boost, ssl, zlib = [source(cache, n) for n in SOURCES]
    out = cache / platform
    out.mkdir(exist_ok=True)
    install = out / 'install'
    prefix = install / 'usr/local/mukhomor-i2p'
    env = os.environ.copy()
    env['PATH'] = '/usr/local/bin:/usr/bin:/bin'
    flags = '-O2 -fPIC -fstack-protector-strong -ffile-prefix-map=' + str(ROOT) + '=/source/mukhomor -ffile-prefix-map='+str(cache)+'=/source/i2p'
    cmake_args = ['-DCMAKE_BUILD_TYPE=Release', '-DCMAKE_POSITION_INDEPENDENT_CODE=ON']
    if platform == 'windows':
        target = 'mingw64'
        env.update(CC='x86_64-w64-mingw32-gcc', CXX='x86_64-w64-mingw32-g++', AR='x86_64-w64-mingw32-ar', RANLIB='x86_64-w64-mingw32-ranlib')
        cmake_args += ['-DCMAKE_SYSTEM_NAME=Windows', '-DCMAKE_C_COMPILER='+env['CC'], '-DCMAKE_CXX_COMPILER='+env['CXX']]
    elif platform.startswith('android-'):
        if not ndk or not (ndk / 'build/cmake/android.toolchain.cmake').exists():
            raise RuntimeError('A Linux Android NDK path is required')
        abi = platform.removeprefix('android-')
        target = {'arm64-v8a':'android-arm64','x86_64':'android-x86_64'}[abi]
        env['ANDROID_NDK_ROOT'] = str(ndk)
        env['PATH'] = str(ndk/'toolchains/llvm/prebuilt/linux-x86_64/bin') + ':' + env['PATH']
        cmake_args += ['-DCMAKE_TOOLCHAIN_FILE='+str(ndk/'build/cmake/android.toolchain.cmake'), '-DANDROID_ABI='+abi, '-DANDROID_PLATFORM=android-26', '-DANDROID_STL=c++_static']
    else:
        target = {'x86_64':'linux-x86_64','aarch64':'linux-aarch64'}[host_platform.machine()]
    def run(args, cwd=out):
        subprocess.run([str(a) for a in args], cwd=cwd, env=env, check=True)
    sslvariant = 'openssl-static-v2-api26' if platform.startswith('android-') else 'openssl-static-v2'
    stamp = out / (sslvariant+'.complete')
    sslbuild = out / sslvariant; sslbuild.mkdir(exist_ok=True)
    if not stamp.exists():
        args = ['perl',ssl/'Configure',target,'no-shared','no-tests','no-apps','no-docs','no-legacy','no-engine','no-module','--prefix=/usr/local/mukhomor-i2p','--openssldir=/etc/ssl','--libdir=lib',flags]
        if platform=='windows': args += ['--cross-compile-prefix=x86_64-w64-mingw32-']
        if platform.startswith('android-'): args += ['-D__ANDROID_API__=26']
        # OpenSSL expects the native compiler name with its cross-prefix option.
        original = {key:env.pop(key) for key in ('CC','CXX','AR','RANLIB') if key in env} if platform=='windows' else {}
        run(args,sslbuild)
        env.update(original)
        run(['make','-j'+str(jobs),'build_generated'],sslbuild)
        run(['make','crypto/buildinf.h'],sslbuild)
        # OpenSSL's informational compiler string embeds command-line flags.
        # Sanitize only this metadata; actual compilation retains path remapping.
        info=sslbuild/'crypto/buildinf.h'
        if info.exists():
            text=info.read_text()
            # mkbuildinf emits a char array, so replace the generated metadata
            # with the public platform/hardening description, not local paths.
            text='#define DATE "built from pinned Mukhomor sources"\n#define PLATFORM "platform: '+target+'"\nstatic const char compiler_flags[] = "compiler: optimized, stack-protector-strong, static OpenSSL";\n'
            info.write_text(text)
        run(['make','-j'+str(jobs)],sslbuild)
        run(['make','install_sw','DESTDIR='+str(install)],sslbuild)
        stamp.write_text(SOURCES['openssl-3.5.9.tar.gz'][0])
    info=sslbuild/'crypto/buildinf.h'
    metadata='#define DATE "built from pinned Mukhomor sources"\n#define PLATFORM "platform: '+target+'"\nstatic const char compiler_flags[] = "compiler: optimized, stack-protector-strong, static OpenSSL";\n'
    if not info.exists() or info.read_text()!=metadata:
        # Also repair an older local cache without weakening the release scan.
        run(['make','crypto/buildinf.h'],sslbuild)
        atomic_write(info,metadata.encode())
        run(['make','-j'+str(jobs)],sslbuild)
        run(['make','install_sw','DESTDIR='+str(install)],sslbuild)
    zb = out/'zlib'
    if not (out/'zlib-static-v2.complete').exists():
        run(['cmake','-S',zlib,'-B',zb,*cmake_args,'-DBUILD_SHARED_LIBS=OFF','-DZLIB_BUILD_TESTING=OFF','-DZLIB_BUILD_SHARED=OFF','-DCMAKE_INSTALL_PREFIX='+str(prefix),'-DCMAKE_C_FLAGS='+flags])
        run(['cmake','--build',zb,'--parallel',str(jobs)])
        run(['cmake','--install',zb])
        (out/'zlib-static-v2.complete').write_text(SOURCES['zlib-1.3.2.tar.gz'][0])
    native=out/'native'
    zlibs=[prefix/'lib'/name for name in ('libz.a','libzlibstatic.a','libzs.a') if (prefix/'lib'/name).is_file()]
    if len(zlibs)!=1: raise RuntimeError('Expected exactly one pinned static zlib')
    run(['cmake','-S',ROOT/'i2p','-B',native,*cmake_args,'-DI2PD_SOURCE='+str(router),'-DBOOST_SOURCE='+str(boost),'-DCMAKE_PREFIX_PATH='+str(prefix),'-DOPENSSL_USE_STATIC_LIBS=TRUE','-DZLIB_USE_STATIC_LIBS=ON','-DOPENSSL_INCLUDE_DIR='+str(prefix/'include'),'-DOPENSSL_CRYPTO_LIBRARY='+str(prefix/'lib/libcrypto.a'),'-DOPENSSL_SSL_LIBRARY='+str(prefix/'lib/libssl.a'),'-DZLIB_INCLUDE_DIR='+str(prefix/'include'),'-DZLIB_LIBRARY='+str(zlibs[0])])
    run(['cmake','--build',native,'--parallel',str(jobs)])
    binary=native/('mukhomor-i2pd.exe' if platform=='windows' else 'mukhomor-i2pd')
    destination=ROOT/'target/i2p-sources'/platform/'native'/binary.name
    if destination!=binary:
        destination.parent.mkdir(parents=True,exist_ok=True)
        atomic_write(destination,binary.read_bytes())
        destination.chmod(0o755)
    return destination


if __name__ == '__main__':
    p=argparse.ArgumentParser()
    p.add_argument('--platform',required=True,choices=['windows','linux','android-arm64-v8a','android-x86_64'])
    p.add_argument('--ndk',type=Path)
    p.add_argument('--jobs',type=int,default=4)
    p.add_argument('--export-dir',type=Path)
    a=p.parse_args()
    binary = build(a.platform,a.ndk,a.jobs)
    if a.export_dir:
        export_artifact(binary, a.platform, a.export_dir)
    print(binary)
