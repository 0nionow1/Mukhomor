<p align="center"><img src="assets/github-banner.svg" alt="Mukhomor — VPN for the web, direct routes for games" width="100%"></p>

# Mukhomor

**A Windows client for Russian gamers who need a VPN without routing their games through it.** Mukhomor adds a native interface, local profile import and split routing around [mihomo](https://github.com/MetaCubeX/mihomo/tree/v1.19.32). The networking core belongs to the mihomo project; Mukhomor is independent of MetaCubeX and Amnezia.

[Русский](README.md) · [Download](https://github.com/0nionow1/Mukhomor/releases/latest) · [Documentation](docs/README.md) · [Issues](https://github.com/0nionow1/Mukhomor/issues)

## Quick start

1. Download `Mukhomor-X.Y.Z-windows-x64.zip` from Releases and extract the **entire** archive.
2. Run `Mukhomor/mukhomor.exe`. Initial setup and updates request administrator permission.
3. Click **Add configuration**, import your server file or paste its share link/text.
4. Select a server and click **Connect**. Add your game's `.exe` under **Bypass rules** if necessary.

No servers, subscriptions or credentials are included. Remote subscription URLs are not fetched; import their contents locally. Windows 10/11 x64 is the supported platform. The executable is currently unsigned; release archives include SHA256 files.

## What it does

Games and matching applications/sites can use your direct connection while other traffic uses the selected VPN server. Presets include game process names, qBittorrent, Russian services, Steam and `.ru`/`.рф`. Direct traffic uses your ISP and exposes your regular public IP. Bypassing a VPN removes that hop; it does not guarantee better latency than your normal connection.

Import supports 21 outgoing types, including WireGuard/AmneziaWG, VLESS, VMess, Trojan, Shadowsocks/SSR, Hysteria 1/2, TUIC and compatible inline OpenVPN profiles. See the [format matrix and limits](docs/protocols.md). HTTP/HTTPS and SSH carry TCP; other protocols' UDP capability depends on server and options. JSON/YAML imports accept standalone nodes, not full Clash configurations.

Closing the window keeps the VPN in the tray. **Exit Mukhomor** disconnects, restores DNS and stops its controller. **Settings → Interface language** switches Auto / Русский / English immediately. Update by extracting a complete new release and running its EXE; installed profiles are retained.

<p align="center"><img src="assets/preview.png" alt="Mukhomor 0.5.0 Windows interface" width="360"></p>

## Learn more

The detailed guides are currently in Russian: [installation](docs/getting-started.md), [gaming rules](docs/gaming-and-routing.md), [VPS setup and official protocol resources](docs/vps-and-vpn.md), [troubleshooting](docs/troubleshooting.md), [architecture](docs/architecture.md), [building](docs/development.md), [releases](docs/releasing.md).

Build on Windows with Rust MSVC, C++ Build Tools, Windows SDK and PowerShell 5.1:

```powershell
.\Fetch-Dependencies.ps1
.\Build-Release.ps1 -Version 0.5.0
```

Own code: [MIT](LICENSE). Bundled mihomo v1.19.32: [GPL-3.0](Mihomo-LICENSE.txt), with its corresponding upstream source archive included in release packages. Wintun binaries, public rules, Tiny5 and Rust dependencies retain their own licenses. See [credits and provenance](docs/credits.md) and [contribution guidelines](CONTRIBUTING.md).
