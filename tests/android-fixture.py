"""Loopback-only SOCKS5 fixture for the Android emulator (10.0.2.2).
No external destinations are contacted. Use only on an isolated test emulator.
"""
import json, socket, socketserver, struct, threading
from pathlib import Path
import sys
counter={"tcp":0,"udp":0}
lock=threading.Lock()
def count(kind):
    with lock:
        counter[kind]+=1
        Path(sys.argv[1]).write_text(json.dumps(counter))
def exact(s,n):
    b=b""
    while len(b)<n:
        v=s.recv(n-len(b))
        if not v: raise EOFError()
        b+=v
    return b
class Socks(socketserver.BaseRequestHandler):
    def handle(self):
        s=self.request;s.settimeout(180)
        try:
            v,n=exact(s,2);exact(s,n);s.sendall(b"\x05\x00")
            v,cmd,_,typ=exact(s,4)
            if typ==1:host=socket.inet_ntoa(exact(s,4))
            elif typ==3:host=exact(s,exact(s,1)[0]).decode()
            elif typ==4:host=socket.inet_ntop(socket.AF_INET6,exact(s,16))
            else:return
            port=struct.unpack("!H",exact(s,2))[0]
            if cmd==3:
                s.sendall(b"\x05\x00\x00\x01"+socket.inet_aton("10.0.2.2")+struct.pack("!H",11081))
                while s.recv(1):pass
            elif cmd==1 and port==18080 and host in ["203.0.113.10","2001:db8::10","fixture.test"]:
                s.sendall(b"\x05\x00\x00\x01"+b"\x00"*6);count("tcp")
                data=s.recv(1024);s.sendall(b"origin:"+data)
            else:s.sendall(b"\x05\x02\x00\x01"+b"\x00"*6)
        except (OSError,EOFError):pass
def udp():
    s=socket.socket(socket.AF_INET,socket.SOCK_DGRAM);s.bind(("127.0.0.1",11081))
    while True:
        b,peer=s.recvfrom(65535)
        if len(b)<10 or b[:3]!=b"\x00\x00\x00":continue
        if b[3]==1 and len(b)>=10:
            host=socket.inet_ntoa(b[4:8]);end=10
        elif b[3]==4 and len(b)>=22:
            host=socket.inet_ntop(socket.AF_INET6,b[4:20]);end=22
        else:continue
        if host not in ["203.0.113.10","2001:db8::10"] or b[end-2:end]!=struct.pack("!H",18080):continue
        count("udp");s.sendto(b[:end]+b"udp:"+b[end:],peer)
threading.Thread(target=udp,daemon=True).start()
socketserver.ThreadingTCPServer.allow_reuse_address=True
server=socketserver.ThreadingTCPServer(("127.0.0.1",11080),Socks)
server.daemon_threads=True
print("READY 127.0.0.1:11080 / UDP 11081",flush=True)
server.serve_forever()
