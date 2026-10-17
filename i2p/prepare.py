"""Verify and prepare upstream i2pd without modifying a shared source cache."""
import hashlib
import io
import os
import tempfile
from pathlib import Path
import tarfile
import urllib.request
import zipfile

ROOT = Path(__file__).resolve().parents[1]
VERSION = "2.61.0"
SHA256 = "409cd3c0257491286611ab6aaf690940c7248fb898377c13fadb65a836e2a0ab"
URL = "https://codeload.github.com/PurpleI2P/i2pd/tar.gz/refs/tags/" + VERSION


def atomic_write(path, data):
    with tempfile.NamedTemporaryFile(dir=path.parent, delete=False) as out:
        temporary = Path(out.name)
        out.write(data)
    try:
        os.replace(temporary, path)
    finally:
        temporary.unlink(missing_ok=True)


def replace_once(text, old, new):
    if text.count(old) != 1:
        raise RuntimeError("Upstream i2pd patch anchor changed")
    return text.replace(old, new)


def patch(name, text):
    if name == "daemon/Daemon.h":
        text = replace_once(text, "#if defined(QT_GUI_LIB) // check if QT", '''#if defined(MUKHOMOR_EMBEDDED)
#define Daemon i2p::util::DaemonEmbedded::Instance()
    class DaemonEmbedded: public Daemon_Singleton {
    public:
        static DaemonEmbedded& Instance() { static DaemonEmbedded instance; return instance; }
        int gracefulShutdownInterval = 0;
    };
#elif defined(QT_GUI_LIB) // check if QT''')
    if name == "libi2pd_client/SOCKS.cpp":
        text = '#include <openssl/crypto.h>\n#include <array>\nextern std::string mukhomor_i2p_token;\n' + text
        text = replace_once(text, 'extern std::string mukhomor_i2p_token;', '#include <string>\nextern std::string mukhomor_i2p_token;')
        text = replace_once(text, "void Handle() override { AsyncSockRead(); }", '''void Handle() override {
                // Private parent protocol: a per-session secret precedes SOCKS.
                // An unauthenticated local client never reaches I2P routing.
                auto self = shared_from_this();
                auto token = std::make_shared<std::array<char, 64>>();
                auto timer = std::make_shared<boost::asio::steady_timer>(m_sock->get_executor());
                timer->expires_after(std::chrono::seconds(5));
                timer->async_wait([self](const boost::system::error_code& e) { if (!e) self->Terminate(); });
                boost::asio::async_read(*m_sock, boost::asio::buffer(*token),
                    [self, token, timer](const boost::system::error_code& e, std::size_t n) {
                        timer->cancel();
                        if (e || n != 64 || mukhomor_i2p_token.size() != 64 ||
                            CRYPTO_memcmp(token->data(), mukhomor_i2p_token.data(), 64)) self->Terminate();
                        else self->AsyncSockRead();
                    });
            }''')
    return text


def prepare(cache=None):
    cache = cache or ROOT / "target/i2p-sources"
    cache.mkdir(parents=True, exist_ok=True)
    archive = cache / ("i2pd-" + VERSION + ".tar.gz")
    if not archive.exists():
        # Reuse the reviewed download, when available.
        candidate = ROOT / "target/i2p-sources" / archive.name
        if not candidate.exists(): candidate = ROOT / "target/p2p-research" / archive.name
        bundle=ROOT/'Source/i2p-dependencies.zip'
        pin=ROOT/'i2p/dependencies.sha256'
        if bundle.exists() and pin.exists():
            if hashlib.sha256(bundle.read_bytes()).hexdigest()!=pin.read_text().split()[0]:
                raise RuntimeError('I2P corresponding source checksum mismatch')
            with zipfile.ZipFile(bundle) as package:
                data=package.read(archive.name)
        else:
            data = candidate.read_bytes() if candidate.exists() else urllib.request.urlopen(URL, timeout=60).read()
        if hashlib.sha256(data).hexdigest() != SHA256:
            raise RuntimeError("i2pd source checksum mismatch")
        atomic_write(archive, data)
    data = archive.read_bytes()
    if hashlib.sha256(data).hexdigest() != SHA256:
        raise RuntimeError("i2pd source checksum mismatch")
    output = cache / ("i2pd-" + VERSION)
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as tar:
        for member in tar.getmembers():
            if not member.isfile():
                continue
            name = member.name.split("/", 1)[1]
            relative = Path(name)
            if relative.is_absolute() or ".." in relative.parts:
                raise RuntimeError("Unsafe source archive entry")
            raw = tar.extractfile(member).read()
            if name in ("daemon/Daemon.h", "libi2pd_client/SOCKS.cpp"):
                raw = patch(name, raw.decode("utf-8")).encode("utf-8")
            dest = output / relative
            dest.parent.mkdir(parents=True, exist_ok=True)
            if not dest.exists() or dest.read_bytes() != raw:
                atomic_write(dest, raw)
    return output


if __name__ == "__main__":
    print(prepare())
