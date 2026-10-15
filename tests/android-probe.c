// Runs as adb shell (outside the VPN app's own excluded UID).
#include <arpa/inet.h>
#include <netdb.h>
#include <stdio.h>
#include <string.h>
#include <sys/socket.h>
#include <unistd.h>
static int exchange(int kind, const char *host, int port, const void *data, int size, unsigned char *out) {
    int family=strchr(host,':')?AF_INET6:AF_INET;
    int fd=socket(family,kind,0); struct timeval timeout={8,0};
    setsockopt(fd,SOL_SOCKET,SO_RCVTIMEO,&timeout,sizeof(timeout));
    setsockopt(fd,SOL_SOCKET,SO_SNDTIMEO,&timeout,sizeof(timeout));
    struct sockaddr_in addr={.sin_family=AF_INET,.sin_port=htons(port)};
    inet_pton(AF_INET,host,&addr.sin_addr);
    struct sockaddr_in6 addr6={.sin6_family=AF_INET6,.sin6_port=htons(port)};
    inet_pton(AF_INET6,host,&addr6.sin6_addr);
    if(connect(fd,family==AF_INET?(void*)&addr:(void*)&addr6,family==AF_INET?sizeof(addr):sizeof(addr6))<0){perror("connect");int mark=0;socklen_t len=sizeof(mark);getsockopt(fd,SOL_SOCKET,SO_MARK,&mark,&len);fprintf(stderr,"uid=%d socket mark=0x%x\n",getuid(),mark);close(fd);return -1;}
    if(send(fd,data,size,0)!=size){perror("send");close(fd);return -1;}
    int n=recv(fd,out,4096,0);close(fd);return n;
}
int main(void) {
    unsigned char out[4096];
    int n=exchange(SOCK_STREAM,"203.0.113.10",18080,"test",4,out);
    if(n!=11||memcmp(out,"origin:test",11)){puts("FAIL TCP");return 1;}
    n=exchange(SOCK_DGRAM,"203.0.113.10",18080,"test",4,out);
    if(n!=8||memcmp(out,"udp:test",8)){puts("FAIL UDP");return 1;}
    const unsigned char dns[]={0,123,1,0,0,1,0,0,0,0,0,0,7,'f','i','x','t','u','r','e',4,'t','e','s','t',0,0,1,0,1};
    n=exchange(SOCK_DGRAM,"9.9.9.9",53,dns,sizeof(dns),out);
    if(n<sizeof(dns)+16||out[0]!=0||out[1]!=123||out[n-4]!=198||out[n-3]!=18){puts("FAIL DNS fake-IP");return 1;}
    char host[INET_ADDRSTRLEN];inet_ntop(AF_INET,out+n-4,host,sizeof(host));
    n=exchange(SOCK_STREAM,host,18080,"test",4,out);
    if(n!=11||memcmp(out,"origin:test",11)){puts("FAIL DNS routing");return 1;}
    n=exchange(SOCK_STREAM,"2001:db8::10",18080,"test",4,out);
    if(n!=11||memcmp(out,"origin:test",11)){puts("FAIL IPv6 TCP");return 1;}
    n=exchange(SOCK_DGRAM,"2001:db8::10",18080,"test",4,out);
    if(n!=8||memcmp(out,"udp:test",8)){puts("FAIL IPv6 UDP");return 1;}
    puts("PASS Android TUN IPv4/IPv6 TCP and UDP, DNS hijack and fake-IP routing");return 0;
}
