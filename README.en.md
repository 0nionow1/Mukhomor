<p align="center"><img src="assets/github-banner.svg" alt="Mukhomor — native VPN client with split routing" width="100%"></p>

# Mukhomor

Version **0.8.1** uses a shared Slint interface. Windows, Linux and Android are required project platforms. See [platform builds, installation and validation](docs/platforms.md); choose the matching platform asset and check its release status.


**A VPN for the services you choose. Direct routes for games.** Mukhomor is a native client for Windows, Linux and Android built around [mihomo v1.19.32](https://github.com/MetaCubeX/mihomo/tree/v1.19.32), with local profile import and configurable split routing. It is independent of MetaCubeX, WireGuard and Amnezia.

[Русский](README.md) · [Download 0.8.1](https://github.com/0nionow1/Mukhomor/releases/tag/v0.8.1) · [Documentation](docs/README.md) · [Full changelog](CHANGELOG.md) · [Issues](https://github.com/0nionow1/Mukhomor/issues)

<p align="center"><img src="assets/preview.png" alt="Mukhomor 0.8.1, Neon theme with synthetic demo servers" width="360"></p>

## Current frontend

Slint is the only UI in this branch. Use `Build-Release.ps1` to build it; previously published archives are not updated automatically. Use `Build-Linux.sh` and `Build-Android.ps1` for the Linux and Android packages. Platform requirements and the validation record are in [platforms.md](docs/platforms.md).

## New in 0.8.1

- Shared Slint frontend for Windows, Linux and Android.
- Linux x64 packages with TUN routing and an installer.
- Android 8+ APK for ARM64/x86_64, VpnService, document import and foreground VPN notification. Release APKs use the project’s permanent signing key.
- Updated website, release catalog and repository artwork in the website’s pixel style.

## Retained from previous releases

- **Redesigned native UI:** 20 themes, Neon by default; three interface sizes (85%, 100%, 125%), a shared pixel mushroom and theme-aware window/tray icons. No WebView.
- **Animated background:** a lava-lamp effect with software rendering. Effects can be disabled.
- **Portable rule presets:** separate Presets and Rules pages, six editable lists, templates and JSON import/export. Exported presets contain rules and metadata without server credentials; imports start disabled with a new identity.
- **Expanded defaults:** 286 gaming processes, 1,123 gaming domain rules and 17 torrent-client processes, alongside Russian services, Steam and Ozon. Domain providers ship offline seeds and refresh every six hours.
- **Independent Windows startup and VPN auto-connect**, safer profile storage and recovery, compact unchanged-status responses and native profile metadata operations.
- **Experimental full routing and WFP traffic protection.** Real Windows VM qualification of leaks, service recovery and reboot behavior is still incomplete; these features are not a qualified kill switch.
- **Experimental Mycelium import and core integration**, including HTTP/2 pooling in matching core builds. Transport capabilities depend on the supplied core and server; parser support for an HTTP/3 option does not establish its availability.

<p align="center"><img src="assets/appearance-preview.png" alt="Mukhomor 0.8.1 theme examples with synthetic demo servers" width="100%"></p>

## Native Slint client

Rust and Slint provide the pixel interface and software rendering without a browser engine. The UI, Windows controller and network core must be measured separately. Previous GDI memory measurements do not describe this frontend. See [measurement guidance](docs/performance.md).

## Roadmap

**Own protocol → P2P → a hosted service.** First stabilize and qualify experimental Mycelium, then develop a P2P protocol, followed by a ready-to-use service for people who find self-managed VPS or P2P inconvenient. No delivery dates or service terms have been announced. See the [roadmap](ROADMAP.md).

## Quick start

Choose the Windows ZIP, Linux tar.gz or Android APK from the release. Linux: extract and run `sudo bash install.sh`, then launch `mukhomor` as a normal user. Android: install the APK and approve the system VPN permission. See [platform requirements and limitations](docs/platforms.md).

Windows:

1. Download `Mukhomor-0.8.1-windows-x64.zip` from [Releases](https://github.com/0nionow1/Mukhomor/releases/tag/v0.8.1) and extract the **entire** archive.
2. Run `Mukhomor/mukhomor.exe`. Initial setup and updates request administrator permission.
3. Click **Add configuration** and import your server file or paste its share link/text.
4. Select a server and connect. Use **Rules** to edit your game's `.exe` bypass if needed; use **Presets** to enable bundled or imported sets.

You need your own server profile. No servers, subscriptions or credentials are included. Remote subscription URLs are not fetched automatically. Executables are currently unsigned; both release archives have SHA256 files.

Closing the window keeps the VPN in the tray. **Exit Mukhomor** disconnects and restores DNS. To update, extract a complete new release and run its EXE; profiles, rules and appearance preferences are retained.

## Routing and presets

In the default split mode, matching games and sites use your direct ISP connection while remaining traffic uses the selected VPN. Direct traffic exposes your normal public IP. Removing the VPN hop does not guarantee lower latency than your ordinary connection; results depend on your ISP and game server.

Preset edits share one draft and one Save action. Existing installations retain their edits and deletions: restore a bundled template or reset rules to adopt expanded defaults. The combined settings limit is 2 MiB; oversized drafts remain in the editor. Saved DNS settings remain intact; advanced Slint controls are tracked in the [remaining work](ui/README.md#remaining-work-and-qualification).

Full mode ignores saved split bypass rules and allows LAN access only when explicitly enabled. Changing routing mode or LAN policy requires disconnecting. Its WFP protection and recovery paths remain experimental pending real Windows VM qualification.

## Startup and appearance

Settings offer independent **Start with Windows** and **Auto-connect VPN** switches. Enable both to open the tray UI at sign-in and connect the last successfully connected server. Merely selecting another server or a failed attempt does not replace that history. Upgrades retain the app's startup registration and respect disabling it in Windows Settings.

Choosing **Exit Mukhomor** suppresses automatic reconnect until the next Windows boot. A subsequent startup invocation in that boot opens only the tray interface; a manual app launch lets you reconnect.

**Settings → Appearance** applies themes, sizes and effects immediately. Appearance is saved separately from VPN configuration. Resetting it preserves your servers and rules. **Interface language** switches Auto / Русский / English.

## Protocols and Mycelium

Local import supports 21 standard outgoing types, including WireGuard/AmneziaWG, VLESS, VMess, Trojan, Shadowsocks/SSR, Hysteria 1/2, TUIC and compatible inline OpenVPN. JSON/YAML accepts standalone nodes, not arbitrary full Clash configurations. UDP support depends on the protocol, options and server. See the [format matrix](docs/protocols.md).

Mycelium requires a separately built core with matching manifest and SHA256. The release builder includes that core and its corresponding source when verified artifacts are present; a standard source build uses stock mihomo. Profiles require an individual nonzero CSPRNG-generated 32-byte seed. TLS verifies certificate identity and validity, including with an explicit certificate pin. UDP over WebSocket runs over TCP and can stall on packet loss. HTTP/3 is build-dependent and experimental. TSPU/DPI resistance has not been established.

## Development and licenses

Detailed guides are in Russian: [installation](docs/getting-started.md), [routing](docs/gaming-and-routing.md), [architecture](docs/architecture.md), [development](docs/development.md), [release process](docs/releasing.md), [troubleshooting](docs/troubleshooting.md).

Build on Windows with Rust MSVC, C++ Build Tools, Windows SDK and PowerShell 5.1:

```powershell
.\Fetch-Dependencies.ps1
.\Build-Release.ps1 -Version 0.8.1
.\tests\Run-Checks.ps1
```

Mukhomor application code is [MIT](LICENSE). Bundled mihomo v1.19.32 is [GPL-3.0](Mihomo-LICENSE.txt), with corresponding source included in release packages. Experimental Mycelium core packages also carry matching modified source. Slint, Wintun, rules, Tiny5 and Rust dependencies retain their own licenses. See [credits](docs/credits.md), [contributing](CONTRIBUTING.md) and [security](SECURITY.md).
