"""Real TCP/UDP/DNS split-routing test in disposable Linux namespaces.

Run as root: python3 tests/linux-network.py /absolute/path/mukhomor-controller
Never changes the host namespace's routes, resolver or active VPN.
"""
import ipaddress
import json
import os
from pathlib import Path
import select
import socket
import socketserver
import struct
import subprocess
import sys
import tempfile
import threading
import time


def exact(sock, size):
    data = b""
    while len(data) < size:
        chunk = sock.recv(size - len(data))
        if not chunk:
            raise EOFError()
        data += chunk
    return data


def address(sock):
    kind = exact(sock, 1)[0]
    if kind == 1:
        host = socket.inet_ntoa(exact(sock, 4))
    elif kind == 3:
        host = exact(sock, exact(sock, 1)[0]).decode()
    elif kind == 4:
        host = socket.inet_ntop(socket.AF_INET6, exact(sock, 16))
    else:
        raise ValueError("Bad SOCKS address")
    port = struct.unpack("!H", exact(sock, 2))[0]
    return ("203.0.113.10" if host.endswith(".test") else host), port


def server(folder):
    counts = {"tcp": 0, "udp": 0}
    lock = threading.Lock()
    def record(kind):
        with lock:
            counts[kind] += 1
            temp = folder / "counts.pending"
            temp.write_text(json.dumps(counts))
            temp.replace(folder / "counts.json")
    class Echo(socketserver.BaseRequestHandler):
        def handle(self):
            self.request.settimeout(8)
            data = self.request.recv(4096)
            self.request.sendall(b"origin:" + data)
    class Socks(socketserver.BaseRequestHandler):
        def handle(self):
            conn = self.request
            conn.settimeout(10)
            version, length = exact(conn, 2)
            assert version == 5
            exact(conn, length)
            conn.sendall(b"\x05\x00")
            version, command, _ = exact(conn, 3)
            dest = address(conn)
            if command == 1:
                record("tcp")
                with socket.create_connection(dest, 5) as upstream:
                    conn.sendall(b"\x05\x00\x00\x01" + socket.inet_aton("192.0.2.2") + struct.pack("!H", 1080))
                    while True:
                        ready, _, _ = select.select([conn, upstream], [], [], 10)
                        if not ready:
                            return
                        for src in ready:
                            data = src.recv(65536)
                            if not data:
                                return
                            (upstream if src is conn else conn).sendall(data)
            elif command == 3:
                conn.sendall(b"\x05\x00\x00\x01" + socket.inet_aton("192.0.2.2") + struct.pack("!H", 1081))
                while conn.recv(4096):
                    pass
    def udp_origin():
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as sock:
            sock.bind(("203.0.113.10", 18080))
            while True:
                packet, peer = sock.recvfrom(65536)
                sock.sendto(b"udp:" + packet, peer)
    def udp_proxy():
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as sock:
            sock.bind(("192.0.2.2", 1081))
            while True:
                packet, peer = sock.recvfrom(65536)
                if packet[:4] != b"\x00\x00\x00\x01":
                    continue
                record("udp")
                dest = socket.inet_ntoa(packet[4:8]), struct.unpack("!H", packet[8:10])[0]
                with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as upstream:
                    upstream.settimeout(5)
                    upstream.sendto(packet[10:], dest)
                    data, _ = upstream.recvfrom(65536)
                    sock.sendto(packet[:10] + data, peer)
    for target in [udp_origin, udp_proxy]:
        threading.Thread(target=target, daemon=True).start()
    for endpoint, handler in [(("203.0.113.10", 18080), Echo), (("192.0.2.2", 1080), Socks)]:
        service = socketserver.ThreadingTCPServer(endpoint, handler)
        service.daemon_threads = True
        threading.Thread(target=service.serve_forever, daemon=True).start()
    (folder / "ready").touch()
    threading.Event().wait()


def client(binary, folder):
    root = folder / "private"
    uid = int(os.environ.get("MUKHOMOR_TEST_UID", "0"))
    def drop_privileges():
        if uid:
            os.setgroups([]); os.setgid(uid); os.setuid(uid)
    process = subprocess.Popen([binary, "--root", str(root)], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, preexec_fn=drop_privileges)
    serial = 0
    def rpc(action, **kwargs):
        nonlocal serial
        serial += 1
        process.stdin.write(json.dumps(dict(action=action, request_id=str(serial), **kwargs)) + "\n")
        process.stdin.flush()
        if not select.select([process.stdout], [], [], 30)[0]:
            raise AssertionError("Controller response timed out")
        reply = json.loads(process.stdout.readline())
        assert reply["ok"], reply.get("error")
        return reply["data"]
    def tcp(host="203.0.113.10"):
        with socket.create_connection((host, 18080), 8) as sock:
            sock.sendall(b"test")
            assert sock.recv(100) == b"origin:test"
    def udp():
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as sock:
            sock.settimeout(8)
            sock.sendto(b"test", ("203.0.113.10", 18080))
            assert sock.recv(100) == b"udp:test"
    def counts():
        path=folder / "counts.json"
        return json.loads(path.read_text()) if path.exists() else {"tcp":0,"udp":0}
    try:
        before = subprocess.check_output(["ip", "rule", "show"], text=True)
        data = rpc("Import", content=json.dumps(dict(name="Network fixture", type="socks5", server="192.0.2.2", port=1080, udp=True)))
        settings = data["settings"]
        settings["routing"] = dict(mode="full", allow_lan=False)
        rpc("ApplySettings", settings=settings)
        rpc("Connect")
        tcp(); udp()
        assert counts()["tcp"] > 0 and counts()["udp"] > 0, "Traffic did not pass through the proxy"
        query = struct.pack("!HHHHHH",123,0x0100,1,0,0,0)+b"\x07fixture\x04test\x00"+struct.pack("!HH",1,1)
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as dns:
            dns.settimeout(8);dns.sendto(query,("9.9.9.9",53));answer=dns.recv(4096)
        assert answer[:2] == struct.pack("!H",123) and struct.unpack("!H",answer[6:8])[0] > 0, "DNS hijack failed"
        fake = socket.inet_ntoa(answer[-4:])
        assert ipaddress.ip_address(fake) in ipaddress.ip_network("198.18.0.0/16"), fake
        tcp(fake)
        rpc("Disconnect")
        assert subprocess.check_output(["ip","rule","show"],text=True) == before, "Disconnect left policy routes"
        # Closing a polled TUN FD wakes its reader asynchronously; wait for the
        # kernel's final reference to disappear, with a strict bounded deadline.
        for _ in range(40):
            if "mukhomor0" not in subprocess.check_output(["ip","link","show"],text=True):break
            time.sleep(.05)
        else:raise AssertionError("Disconnect kept the TUN device")
        # Identical reconnect must carry traffic again, not merely report an
        # enabled cached config after the underlying listener has been closed.
        baseline=counts();rpc("Connect");tcp();udp()
        assert counts()["tcp"]>baseline["tcp"] and counts()["udp"]>baseline["udp"],"Reconnect reused a closed TUN listener"
        rpc("Disconnect")
        settings["routing"]["mode"]="split"
        settings["direct"]["ip_cidrs"]=["203.0.113.10/32"]
        rpc("ApplySettings",settings=settings);rpc("Connect")
        baseline=counts();tcp();udp();assert counts()==baseline,"Direct exceptions went through VPN"
        rpc("Disconnect");rpc("Connect")
        assert "mukhomor0" in subprocess.check_output(["ip","link","show"],text=True),"Repeated connect did not create TUN"
        rpc("Disconnect")
        assert subprocess.check_output(["ip","rule","show"],text=True)==before
        rpc("Exit");process.wait(timeout=8)
        print("PASS: real TUN TCP, UDP, DNS hijack/fake-IP, split routing, repeated connect/disconnect and route cleanup")
    finally:
        if process.poll() is None:
            process.terminate()
            try: process.wait(timeout=8)
            except subprocess.TimeoutExpired: process.kill();process.wait()


def run(binary):
    if os.geteuid()!=0: raise SystemExit("Run the namespace test as root")
    token=str(os.getpid())
    client_ns="mukh-c-"+token;server_ns="mukh-s-"+token
    namespaces=[];fixture=None
    def ip(*args): subprocess.run(["ip",*args],check=True,stdout=subprocess.DEVNULL)
    with tempfile.TemporaryDirectory(prefix="mukhomor-network-") as temp:
        folder=Path(temp)
        if os.environ.get("MUKHOMOR_TEST_UID"):
            os.chown(folder,int(os.environ["MUKHOMOR_TEST_UID"]),int(os.environ["MUKHOMOR_TEST_UID"]))
        try:
            for ns in [client_ns,server_ns]:ip("netns","add",ns);namespaces.append(ns)
            ip("link","add","mc"+token,"type","veth","peer","name","ms"+token)
            ip("link","set","mc"+token,"netns",client_ns);ip("link","set","ms"+token,"netns",server_ns)
            for ns,device,address in [(client_ns,"mc"+token,"192.0.2.1/24"),(server_ns,"ms"+token,"192.0.2.2/24")]:
                ip("-n",ns,"link","set","lo","up");ip("-n",ns,"addr","add",address,"dev",device);ip("-n",ns,"link","set",device,"up")
            ip("-n",client_ns,"route","add","default","via","192.0.2.2")
            ip("-n",server_ns,"addr","add","203.0.113.10/32","dev","lo")
            fixture=subprocess.Popen(["ip","netns","exec",server_ns,sys.executable,__file__,"server",temp])
            for _ in range(100):
                if (folder/"ready").exists():break
                if fixture.poll() is not None:raise RuntimeError("Network fixture exited")
                time.sleep(.05)
            subprocess.run(["ip","netns","exec",client_ns,sys.executable,__file__,"client",str(Path(binary).resolve()),temp],check=True,timeout=180)
        finally:
            if fixture is not None:fixture.terminate();fixture.wait(timeout=5)
            for ns in reversed(namespaces):subprocess.run(["ip","netns","del",ns],check=False)

if __name__=="__main__":
    if sys.argv[1]=="server":server(Path(sys.argv[2]))
    elif sys.argv[1]=="client":client(sys.argv[2],Path(sys.argv[3]))
    else:run(sys.argv[1])
