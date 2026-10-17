"""Authenticated mock Tor transport for real, isolated Linux TUN tests.

This verifies integration and failure handling, not the public Tor network.
"""
import json
import os
from pathlib import Path
import select
import shlex
import shutil
import signal
import socket
import struct
import subprocess
import sys
import threading
import time


def exact(sock, size):
    data = b""
    while len(data) < size:
        piece = sock.recv(size-len(data))
        if not piece:
            raise EOFError()
        data += piece
    return data


def helper():
    init = json.loads(sys.stdin.readline())
    def lifetime():
        sys.stdin.read(1)
        os._exit(0)
    threading.Thread(target=lifetime, daemon=True).start()
    (Path(init["state_dir"]).parent / "fixture.pid").write_text(str(os.getpid()))
    if (Path(init["state_dir"]).parent / "fixture.fail").exists():
        time.sleep(2)
        raise SystemExit(1)
    def connection(client):
        with client:
            client.settimeout(5)
            assert exact(client,1) == b"\x05"
            assert 2 in exact(client,exact(client,1)[0])
            client.sendall(b"\x05\x02")
            assert exact(client,1) == b"\x01"
            user=exact(client,exact(client,1)[0]).decode()
            password=exact(client,exact(client,1)[0]).decode()
            assert user == init.get("username",init.get("token")) and password == init["password"]
            client.sendall(b"\x01\x00")
            header=exact(client,4)
            assert header[:3] == b"\x05\x01\x00", "UDP reached the Tor transport"
            kind=header[3]
            if kind == 1: destination=exact(client,4)
            elif kind == 4: destination=exact(client,16)
            else:
                length=exact(client,1);destination=length+exact(client,length[0])
            destination+=exact(client,2)
            with socket.socket() as outer:
                outer.setsockopt(socket.SOL_SOCKET,socket.SO_BINDTODEVICE,init["interface"].encode()+b"\0")
                outer.setsockopt(socket.SOL_SOCKET,socket.SO_MARK,0x4d544f52)
                outer.settimeout(5);outer.connect(("192.0.2.2",1080))
                outer.sendall(b"\x05\x01\x00");assert exact(outer,2)==b"\x05\x00"
                outer.sendall(header+destination);client.sendall(exact(outer,10))
                while True:
                    readable,_,_=select.select([client,outer],[],[],5)
                    if not readable:return
                    for source in readable:
                        chunk=source.recv(65536)
                        if not chunk:return
                        (outer if source is client else client).sendall(chunk)
    with socket.socket() as listener:
        listener.bind(("127.0.0.1",0));listener.listen()
        print(json.dumps(dict(ready=True,port=listener.getsockname()[1])),flush=True)
        while True:
            client,_=listener.accept()
            threading.Thread(target=connection,args=(client,),daemon=True).start()


def prepare(binary, folder, network="tor"):
    bundle=folder/"bundle";bundle.mkdir()
    target=bundle/"mukhomor-controller"
    shutil.copy2(binary,target)
    wrapper=bundle/"mukhomor-tor"
    wrapper.write_text("#!/bin/sh\nexec "+shlex.quote(sys.executable)+" "+shlex.quote(str(Path(__file__).resolve()))+" --helper\n")
    wrapper.chmod(0o755)
    if network == "i2p":
        shutil.copy2(wrapper,bundle/"mukhomor-pt")
    return target


def client(binary, folder, network="tor"):
    root=folder/"private"
    process=subprocess.Popen([binary,"--root",str(root)],stdin=subprocess.PIPE,stdout=subprocess.PIPE,text=True)
    serial=0
    def rpc(action,expect_ok=True,**kwargs):
        nonlocal serial
        serial+=1
        process.stdin.write(json.dumps(dict(action=action,request_id=str(serial),**kwargs))+"\n");process.stdin.flush()
        assert select.select([process.stdout],[],[],15)[0], "controller timed out"
        reply=json.loads(process.stdout.readline());assert bool(reply["ok"])==expect_ok,reply.get("error")
        return reply["data"]
    def counts():return json.loads((folder/"counts.json").read_text())
    def tcp():
        host="203.0.113.10"
        if network=="i2p":
            query=struct.pack("!HHHHHH",123,0x0100,1,0,0,0)+b"\x07fixture\x03i2p\x00"+struct.pack("!HH",1,1)
            with socket.socket(socket.AF_INET,socket.SOCK_DGRAM) as dns:
                dns.settimeout(3);dns.sendto(query,("1.1.1.1",53));reply=dns.recv(2048)
                assert reply[:2]==query[:2] and struct.unpack("!H",reply[6:8])[0]>0,"I2P fake DNS failed"
                host=socket.inet_ntoa(reply[-4:]);assert host.startswith("198.18."),"I2P leaked DNS"
        with socket.create_connection((host,18080),3) as stream:
            stream.sendall(b"test");assert stream.recv(100)==b"origin:test"
    def blocked_udp():
        with socket.socket(socket.AF_INET,socket.SOCK_DGRAM) as stream:
            stream.settimeout(.7)
            try:
                stream.sendto(b"test",("203.0.113.10",18080));stream.recv(100)
            except (TimeoutError,ConnectionRefusedError,PermissionError):return
            raise AssertionError("Tor mode leaked UDP")
    interface=subprocess.check_output(["ip","route","show","default"],text=True).split("dev ")[1].split()[0]
    def physical_tcp(blocked):
        for host in ["203.0.113.10","2001:db8::10"]:
            for port in [18080,53,853]:
                with socket.socket(socket.AF_INET6 if ":" in host else socket.AF_INET) as stream:
                    stream.setsockopt(socket.SOL_SOCKET,socket.SO_BINDTODEVICE,interface.encode()+b"\0")
                    if port in [53,853]:stream.setsockopt(socket.SOL_SOCKET,socket.SO_MARK,0x4d544f52)
                    stream.settimeout(.25)
                    try:
                        stream.connect((host,port));stream.sendall(b"test")
                        assert stream.recv(100)==b"origin:test"
                    except (OSError,AssertionError):
                        if blocked:continue
                        raise
                    assert not blocked,"Tor guard allowed a physical TCP/DNS bypass"
            for port in [53,853,18080]:
                for marked in [False,True]:
                    allowed=not blocked or (marked and port==18080)
                    with socket.socket(socket.AF_INET6 if ":" in host else socket.AF_INET,socket.SOCK_DGRAM) as dns:
                        dns.setsockopt(socket.SOL_SOCKET,socket.SO_BINDTODEVICE,interface.encode()+b"\0")
                        if marked:dns.setsockopt(socket.SOL_SOCKET,socket.SO_MARK,0x4d505455)
                        dns.settimeout(.25)
                        try:
                            dns.sendto(b"dns-test",(host,port));assert dns.recv(100)==b"udp:dns-test"
                        except (OSError,AssertionError):
                            if not allowed:continue
                            raise
                        assert allowed,"Tor guard allowed physical user UDP or marked DNS"
    try:
        before=subprocess.check_output(["ip","rule","show"])
        data=rpc("Status");settings=data["settings"]
        settings["routing"]=dict(mode="split",allow_lan=True)
        settings["direct"]["ip_cidrs"]=["203.0.113.10/32"]
        rpc("ApplySettings",settings=settings)
        physical_tcp(False)
        # Failed bootstrap must already be protected, before any TUN exists.
        (root/network).mkdir(mode=0o700,exist_ok=True)
        failure=root/network/"fixture.fail";failure.touch()
        process.stdin.write(json.dumps(dict(action="Connect",network=network))+"\n");process.stdin.flush()
        for _ in range(80):
            if (root/network/"fixture.pid").exists():break
            time.sleep(.025)
        else:raise AssertionError("Bootstrap fixture did not start")
        physical_tcp(True);blocked_udp()
        assert select.select([process.stdout],[],[],10)[0]
        response=json.loads(process.stdout.readline());assert not response["ok"]
        assert response["data"]["can_disconnect"]
        physical_tcp(True);blocked_udp()
        failure.unlink()
        rpc("Disconnect");physical_tcp(False)
        data=rpc("Connect",network=network)
        assert data["network"]==network and data["running"] and not data["profiles"]
        tcp();baseline=counts();assert baseline["tcp"]>0
        if network=="i2p":
            try:
                with socket.create_connection(("203.0.113.10",18080),1) as stream:
                    stream.sendall(b"external"); assert not stream.recv(100),"I2P leaked an IP destination"
            except (OSError,ConnectionError):pass
            assert counts()==baseline,"I2P forwarded an external destination"
        rpc("Connect",network="tor" if network=="i2p" else "i2p",expect_ok=False)
        physical_tcp(True)
        baseline=counts()
        blocked_udp();assert counts()==baseline
        assert data["settings"]==settings, "Tor overwrote split preferences"
        pid=int((root/network/"fixture.pid").read_text());os.kill(pid,signal.SIGKILL)
        for _ in range(40):
            data=rpc("Status")
            if data["phase"]=="recovery":break
            time.sleep(.05)
        assert not data["running"] and data["can_disconnect"] and data["phase"]=="recovery"
        assert "mukhomor0" in subprocess.check_output(["ip","link","show"],text=True)
        try:tcp()
        except (OSError,AssertionError):pass
        else:raise AssertionError("Dead Tor transport fell back to DIRECT")
        blocked_udp();assert counts()==baseline
        rpc("Disconnect");assert subprocess.check_output(["ip","rule","show"])==before
        rpc("Connect",network=network);tcp();assert counts()["tcp"]>baseline["tcp"]
        # Killing the whole controller removes TUN, but not the kernel guard.
        process.kill();process.wait(timeout=5)
        physical_tcp(True);blocked_udp()
        process=subprocess.Popen([binary,"--root",str(root)],stdin=subprocess.PIPE,stdout=subprocess.PIPE,text=True)
        recovered=rpc("Status")
        assert recovered["network"]==network and recovered["phase"]=="recovery" and recovered["can_disconnect"]
        rpc("Connect",network="vps",expect_ok=False)
        physical_tcp(True)
        rpc("Disconnect");physical_tcp(False)
        rpc("Exit");process.wait(timeout=5)
        print("PASS: Linux "+network+" TUN; bootstrap failure, physical IPv4/IPv6 TCP and DNS (including marked 53/853), UDP, helper crash and controller SIGKILL all blocked; persisted guard recovered; explicit disconnect restores direct access")
        print("Public P2P cryptography/network availability is not tested by this fixture.")
    finally:
        if process.poll() is None:
            process.terminate()
            try:process.wait(timeout=5)
            except subprocess.TimeoutExpired:process.kill();process.wait()


if __name__=="__main__" and sys.argv[1:]==["--helper"]:
    helper()
