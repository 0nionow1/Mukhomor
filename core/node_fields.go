// Protocol allowlist shared with ProfileImport.psm1. No global configuration.
package core

var nodeFields = map[string]string{
	"mycelium":    "seed sni fingerprint client-fingerprint udp transport udp-transport",
	"wireguard":   "ip ipv6 private-key public-key pre-shared-key reserved allowed-ips workers mtu udp persistent-keepalive amnezia-wg-option ip-stack remote-dns-resolve dns refresh-server-ip-interval",
	"ss":          "password cipher udp plugin plugin-opts udp-over-tcp udp-over-tcp-version client-fingerprint",
	"ssr":         "password cipher obfs obfs-param protocol protocol-param udp",
	"vmess":       "uuid alterId cipher udp tls global-padding authenticated-length servername client-fingerprint alpn skip-cert-verify name-cert-verify fingerprint certificate private-key network ws-opts http-opts h2-opts grpc-opts reality-opts packet-addr xudp packet-encoding",
	"vless":       "uuid flow encryption udp tls ws-headers xhttp-opts servername client-fingerprint alpn skip-cert-verify name-cert-verify fingerprint certificate private-key network ws-opts http-opts h2-opts grpc-opts reality-opts packet-addr xudp packet-encoding",
	"trojan":      "password udp network ws-opts grpc-opts reality-opts ss-opts sni client-fingerprint alpn skip-cert-verify name-cert-verify fingerprint certificate private-key",
	"hysteria":    "ports protocol obfs-protocol up up-speed down down-speed auth auth-str obfs recv-window-conn recv-window disable-mtu-discovery fast-open hop-interval sni alpn skip-cert-verify name-cert-verify fingerprint certificate private-key",
	"hysteria2":   "ports hop-interval up down password obfs obfs-password obfs-min-packet-size obfs-max-packet-size cwnd bbr-profile udp-mtu handshake-timeout initial-stream-receive-window max-stream-receive-window initial-connection-receive-window max-connection-receive-window sni alpn skip-cert-verify name-cert-verify fingerprint certificate private-key",
	"tuic":        "token uuid password ip heartbeat-interval reduce-rtt request-timeout udp-relay-mode congestion-controller disable-sni max-udp-relay-packet-size fast-open max-open-streams cwnd bbr-profile recv-window-conn recv-window disable-mtu-discovery max-datagram-frame-size udp-over-stream udp-over-stream-version sni alpn skip-cert-verify name-cert-verify fingerprint certificate private-key",
	"snell":       "psk udp version reuse obfs-opts client-fingerprint",
	"socks5":      "username password tls udp skip-cert-verify name-cert-verify fingerprint certificate private-key",
	"http":        "username password tls headers sni skip-cert-verify name-cert-verify fingerprint certificate private-key",
	"ssh":         "username password private-key private-key-passphrase host-key host-key-algorithms",
	"anytls":      "password udp client-metadata idle-session-check-interval idle-session-timeout min-idle-session disable-reuse sni client-fingerprint alpn skip-cert-verify name-cert-verify fingerprint certificate private-key",
	"mieru":       "port-range transport udp username password multiplexing handshake-mode traffic-pattern",
	"trusttunnel": "username password udp health-check quic congestion-controller cwnd bbr-profile max-connections min-streams max-streams sni client-fingerprint alpn skip-cert-verify name-cert-verify fingerprint certificate private-key",
	"shadowquic":  "username password sni alpn quic-versions udp-over-stream zero-rtt keep-alive-interval congestion-controller up down cwnd bbr-profile recv-window-conn recv-window disable-mtu-discovery max-datagram-frame-size max-open-streams",
	"gost-relay":  "username password forward udp tls mux sni skip-cert-verify name-cert-verify fingerprint certificate private-key client-fingerprint",
	"sudoku":      "key aead-method padding-min padding-max table-type enable-pure-downlink http-mask http-mask-mode http-mask-tls http-mask-host path-root multiplex http-mask-multiplex httpmask custom-table custom-tables",
	"masque":      "private-key public-key ip ipv6 uri sni mtu udp handshake-timeout skip-cert-verify network congestion-controller cwnd bbr-profile ip-stack remote-dns-resolve dns",
	"openvpn":     "proto dev cipher data-ciphers data-ciphers-fallback auth comp-lzo ca cert key tls-auth key-direction tls-crypt tls-crypt-v2 username password peer-info ping ping-restart tran-window handshake-timeout mtu udp ip-stack remote-dns-resolve dns",
}
