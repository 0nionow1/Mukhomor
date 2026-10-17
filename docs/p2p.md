# P2P connectors

Development version: 0.8.8. Research reviewed on 2026-10-17.
This is a local development build, not a published release. No network can
guarantee anonymity, trustworthy content, availability, or protection from a
compromised device. The list contains distinct networks, not a count of bridge
types. Tor bridges all reach the same Tor network.

| Connector | Scope | Account / payment | Implementation |
| --- | --- | --- | --- |
| Tor | Internet TCP and onion services supported by the transport | Neither | Arti; direct, obfs4, WebTunnel, Snowflake |
| I2P | Internal `.i2p` TCP services only | Neither | Client-only i2pd, NTCP2, three-hop inbound/outbound tunnels |

Select a network in **P2P**, then connect. Disconnect before changing the active
network. I2P intentionally blocks ordinary Internet destinations; it is not a
fallback exit for Internet browsing. Application UDP is blocked in both modes.
Saved VPS exceptions, LAN bypasses and Android app exclusions do not apply.
Tor retains HTTP compatibility, with an explicit HTTPS warning. See
[Tor implementation and acceptance](tor.md).

## I2P protection

The parent owns `mukhomor-pt`, which owns `mukhomor-i2pd`, through anonymous stdin
pipes. Session secrets are never command-line arguments. Public SOCKS requires
per-session authentication; it accepts domain CONNECT requests ending in `.i2p`
and rejects IP literals, UDP and ordinary Internet domains. The inner i2pd SOCKS
socket also requires a secret before its protocol. Both listen only on loopback.

The native child has no direct network exception. Its NTCP2 traffic goes through
an authenticated, public-IP-only CONNECT adapter in the bound Go transport.
SSU2, transit routing, floodfill, HTTP proxy, admin console, SAM, BOB, I2CP,
I2PControl, UPnP and network time synchronization are disabled. No public exit or
incoming application service is offered. Initial router information is fetched
over verified HTTPS through the bound adapter, then checked with mandatory SU3
signature verification using the pinned upstream signer certificates.

Mihomo routes only `.i2p` to the authenticated SOCKS endpoint. Fake-IP mapping
does not consult ordinary DNS. Configured resolvers return an empty response;
the required literal bootstrap resolver is confined to loopback discard port 1.
Sniffing is disabled. `.b32.i2p` addresses identify a destination cryptographically;
human-readable `.i2p` names additionally depend on the address book. Applications,
browser fingerprints, logins and downloaded documents can still reveal identity.

Failure of the child closes the proxy. Existing Windows/Linux traffic protection
or the Android blocking TUN must remain until an explicit disconnect. Android's
own application UID is excluded from its VPN so the bundled transports can reach
peers; other user-selected application exclusions are ignored in P2P mode.

## Candidate review

The selection requires free sustained use, no account, an actual decentralized
network, and a defensible integration with the supported platforms. This is a
review of the candidates below, not a claim to have discovered every network.

| Candidate | Decision and primary source |
| --- | --- |
| Tor | Included. Volunteer onion-routing network; bridge transports provide alternative ways to reach it. [Tor documentation](https://support.torproject.org/) |
| I2P | Included as an internal network; external outproxies are deliberately disabled. [I2P FAQ](https://geti2p.net/en/faq), [i2pd](https://github.com/PurpleI2P/i2pd) |
| Lokinet | Not offered as working. The pinned Linux v0.9.14 probe with bundled bootstrap failed; a corrected probe using the official `lokinet.signed` seed built network paths but did not pass exit HTTPS in 320 seconds. The initial root seed URL was an informational HTML page, not a network failure. These local results do not prove global unavailability. Current cross-platform support and a reliable free exit still need qualification. [Stable release](https://github.com/oxen-io/lokinet/releases/tag/v0.9.14), [seed server](https://seed.lokinet.org/), [exit nodes](https://docs.oxen.io/oxen-docs/products-built-on-oxen/lokinet/exit-nodes) |
| BelNet | Candidate, not integrated or declared unsafe. Its official site offers account-free onion routing and community exits, but the reviewed native source still defaults to OpenSSL 3.0.7 and curl 7.86.0. Dependency refresh, reproducible platform builds and leak/exit qualification remain necessary before offering it as a protected connector. No BelNet network acceptance test was performed. [Project](https://belnet.beldex.io/), [reviewed build dependencies](https://github.com/Beldex-Coin/belnet/blob/480aafb3c68ac359f0711b2cbd8a07be661703c0/cmake/StaticBuild.cmake) |
| Yggdrasil | Encrypted IPv6 overlay, explicitly not an anonymity network. Peer IP visibility and its experimental security status do not meet this privacy connector's requirements. [FAQ](https://yggdrasil-network.github.io/faq.html) |
| cjdns | Encrypted overlay, not designed to anonymize traffic. No qualified free anonymous Internet exit included. [Project FAQ](https://github.com/cjdelisle/cjdns/blob/master/doc/faq/general.md) |
| GNUnet VPN | Its own documentation warns that peers can see packet source/destination and that the VPN does not provide good anonymity; currently GNU/Linux only. Not selected for this privacy mode. [VPN manual](https://docs.gnunet.org/latest/users/vpn.html) |
| Hyphanet | Separate publishing/storage network, not a TCP/IP proxy. Needs a content application integration rather than the VPN connector implemented here; not included. [Help](https://www.hyphanet.org/pages/help.html) |
| Current Freenet | Decentralized application platform which explicitly should not be relied on as an anonymity system. Not included. [FAQ](https://freenet.org/about/faq/) |
| Mysterium / Orchid / Sentinel / Nym | Paid bandwidth, marketplace or time-limited trial does not meet sustained free use. [Mysterium](https://www.mysteriumvpn.com/blog/paid-vs-free-vpn), [Orchid](https://www.orchid.com/vpn/), [Sentinel](https://docs.sentinel.co/), [Nym pricing](https://nym.com/pricing) |
| Proton / Psiphon / Lantern | Not selected: centralized services are outside the user's P2P-only scope; Proton also requires an account. [Proton configuration](https://protonvpn.com/support/wireguard-configurations), [Psiphon](https://psiphon.ca/en/why-psiphon), [Lantern FAQ](https://lantern.io/faq) |
| VPN Gate | Volunteer gateways retain connection logs; not selected for this privacy mode. [Logging policy](https://www.vpngate.net/en/about_abuse.aspx) |

## Build inputs and validation

i2pd 2.61.0, Boost 1.90.0, OpenSSL 3.5.9 and zlib 1.3.2 are pinned by SHA-256.
`i2p/prepare.py` applies narrow ownership/authentication patches. The corresponding
source archives and license notices accompany packages. Windows builds use
MinGW in WSL Ubuntu; Android builds additionally need Linux NDK r27c at
`target/i2p-sources/android-ndk-r27c` (or `-LinuxNdk` for the helper build script).
Set `MUKHOMOR_LINUX_NDK` to override that location for the complete Android build.
On WSL, compilation uses an ext4 cache by default; `MUKHOMOR_I2P_BUILD_CACHE`
can override it. Final helper binaries are copied back into the project target.
Linux uses GCC/CMake. Network access is needed only for missing pinned sources.

GitHub Actions builds the Windows and both Android I2P children on Linux, then
passes them to the Windows packaging jobs through artifacts from the same run.
`MUKHOMOR_I2P_PREBUILT_ROOT` selects this CI input; the wrapper verifies platform,
binary size and SHA-256, plus the exact reviewed build-input hashes before copying.
An absent or mismatched manifest fails the build. Ordinary local builds keep the
Linux/WSL compilation path.

Acceptance on 2026-10-17:

- Windows: the production helpers reached an actual `.b32.i2p` HTTP resource
  through authenticated SOCKS in 73 seconds, and in 93 seconds on the crash-test
  run. Ordinary Internet destinations were refused. Killing the owned native
  child closed both the proxy and its supervisor. Configurations were accepted
  by bundled Mihomo; guard-plan, stale-helper, cancellation and process-pipe
  tests passed without changing host networking. A separate loopback HTTP test
  verifies destination warmup retries within one deadline and prompt cancellation.
- Linux: real I2P traffic passed through the VPN TUN and fake DNS in a disposable
  network namespace (28 seconds on the complete final run). Physical-interface
  TCP was blocked. Killing the adapter retained protection; explicit disconnect
  restored the control connection. Additional mock-transport namespace tests
  exercised IPv4/IPv6, UDP/DNS, bootstrap failure, controller death and recovery.
  Installer rollback checks covered all five binaries and preserved profiles.
- Android: an Android 15 x86_64 emulator opened the real `.b32.i2p` resource
  through the application TUN and fake DNS. Killing the native helper terminated
  its supervisor while retaining the TUN and blocking TCP from a separate UID;
  explicit disconnect restored the control connection. This is emulator
  qualification, not a physical ARM-device test.

Destination reachability varied between attempts; local tunnel readiness does
not guarantee that a remote site is immediately reachable. These successful
connections do not guarantee future availability or qualify every application.
Host Windows installation, full WFP activation alongside the user's active
Amnezia, and physical Android hardware remain untested. The isolated probes do
not change either installed host VPN.

Follow-up on 2026-10-17: an isolated Windows physical-interface I2P probe reached
the same real destination in 186 seconds; ordinary Internet was refused and
native-child failure closed the proxy. This exposed the separate 90-second
Windows health deadline as too short: bootstrap and destination warmup now share
one eight-minute budget. Full host WFP/TUN coexistence is still not qualified.
The VPS probe reached I2P in 54 seconds. Its first attempt failed before network
startup because the Ubuntu 22.04 glibc was older than the native helper requires.
The successful retry used a private compatible runtime; no system libraries were
replaced. Both probe directories, archives, runtimes and processes were removed
and cleanup was verified. The Linux installer now checks for glibc 2.38+ before
changing files. This is an availability record, not a guarantee for every ISP.
