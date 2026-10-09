# Источники и лицензии

Mukhomor — независимый клиент с интерфейсом и управлением разделением трафика. Сетевые протоколы и маршрутизацию реализует **mihomo**. Проект не является официальным продуктом MetaCubeX, WireGuard или Amnezia.

## Компоненты

| Компонент | Как используется | Источник и лицензия |
| --- | --- | --- |
| Mukhomor | Rust/Win32 интерфейс, контроллер, импорт и PowerShell управление Windows | [MIT](../LICENSE); сохранено упоминание исходных VPN-Split contributors |
| mihomo **v1.19.32** | Неизменённый отдельный процесс: протоколы, DNS, TUN и правила | [MetaCubeX/mihomo, закреплённый тег](https://github.com/MetaCubeX/mihomo/tree/v1.19.32), [GPL-3.0](../Mihomo-LICENSE.txt) |
| Wintun **0.14.1** | Официальная неизменённая amd64 DLL для TUN | [wintun.net](https://www.wintun.net/), [Prebuilt Binaries License](../Wintun-LICENSE.txt) |
| meta-rules-dat | Начальные списки Россия, Steam, Ozon; сортировка и удаление точных дублей | [MetaCubeX/meta-rules-dat](https://github.com/MetaCubeX/meta-rules-dat), [GPL-3.0](../Rules-LICENSE.txt), [происхождение списков](../lists/sources.json) |
| Tiny5 Regular | Встроенный пиксельный шрифт с кириллицей | [Google Fonts](https://github.com/google/fonts/tree/main/ofl/tiny5), [SIL OFL 1.1](../Tiny5-LICENSE.txt), [SHA256](../assets/fonts/README.md) |
| Rust crates | JSON, Win32 bindings и транзитивные зависимости | [Cargo.lock](https://github.com/0nionow1/Mukhomor/blob/main/native/Cargo.lock), тексты в [ThirdParty-LICENSE.txt](../ThirdParty-LICENSE.txt) |

Версии, адреса загрузки и SHA256 ядра, Wintun и исходного архива закреплены в [Fetch-Dependencies.ps1](../Fetch-Dependencies.ps1) и проверяются при сборке. Точная версия ядра важнее лицензии или описания текущей ветки upstream.

MIT относится к собственному коду Mukhomor. Компоненты релиза сохраняют отдельные лицензии; весь Windows ZIP нельзя описывать как «только MIT». Архив соответствующих исходников mihomo поставляется в `Source/mihomo-v1.19.32-source.zip` внутри Windows и source пакетов. При распространении сохраняйте лицензии и доступ к соответствующим исходникам. Wintun поставляется под лицензией готовых бинарных файлов, а не под MIT Mukhomor.

## Сетевые источники

Основной DNS в сгенерированной конфигурации использует DoH Cloudflare и Google. Дополнительное обновление IP через JSON API выключено по умолчанию. Включённые публичные списки могут обращаться к GitHub для обновления. Эти обращения — часть работы DNS и списков, не собственный сервис аналитики Mukhomor.

- [Cloudflare DNS JSON API](https://developers.cloudflare.com/1.1.1.1/encryption/dns-over-https/make-api-requests/dns-json/).
- [Google DNS JSON API](https://developers.google.com/speed/public-dns/docs/doh/json).
- [Общие параметры mihomo](https://wiki.metacubex.one/en/config/proxies/), [TUN](https://wiki.metacubex.one/en/config/inbound/tun/), [правила](https://wiki.metacubex.one/en/config/rules/).

Ссылки для развёртывания серверов собраны в [VPS и VPN](vps-and-vpn.md), для каждого импортируемого типа — в [Протоколах](protocols.md). Упоминание серверного проекта не означает, что его код включён в Mukhomor. Баннер создаётся из собственной палитры и пиксельного мотива приложения; скриншот показывает существующий интерфейс.
