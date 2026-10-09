# Свой VPS и VPN

Mukhomor — приложение на вашем Windows-компьютере. Оно подключается к уже работающему серверу. VPS арендуется и настраивается отдельно; SSH-реквизиты VPS в Mukhomor вводить не нужно.

## Выбор VPS

Оцените расположение относительно вашего провайдера, публичный IP, доступный трафик, поддержку нужных TCP/UDP портов и правила хостинга. Сравнивайте пробное соединение, потери пакетов и маршрут, а не только расстояние на карте. Игры из исключений всё равно идут напрямую и не зависят от расположения этого VPS.

Для установки через Amnezia ориентируйтесь на её актуальные [требования к VPS](https://docs.amnezia.org/documentation/supported-linux-os-for-vps/) и [стартовое руководство](https://amnezia.org/starter-guide). В нём описаны KVM, публичный IPv4, Linux x86-64 и от 1 ГБ RAM. Для другого серверного ПО сверяйте его собственные требования.

## Простой путь: AmneziaWG

1. Получите Linux VPS и доступ по SSH.
2. Установите официальную AmneziaVPN и воспользуйтесь [инструкцией установки VPN на свой сервер](https://docs.amnezia.org/documentation/instructions/install-vpn-on-server/).
3. Настройте AmneziaWG и создайте отдельный клиентский профиль. Экспортируйте совместимый `.conf` для WireGuard/AmneziaWG, а не резервную копию приложения и не файл полного доступа к VPS.
4. Завершите соединение AmneziaVPN в Windows, импортируйте `.conf` в Mukhomor и подключитесь.
5. Проверьте сайт через VPN, затем игру по прямому исключению.

Версии AmneziaWG развиваются отдельно. Поддержка определяется параметрами `.conf`, которые понимают **mihomo v1.19.32** и импортёр Mukhomor. Не каждый экспорт новой Amnezia автоматически совместим. При ошибке используйте [параметры WireGuard в mihomo](https://wiki.metacubex.one/en/config/proxies/wg/) и [ограничения импорта](protocols.md).

## VLESS / Reality / VMess / Trojan

Сервер может работать на Xray. Для обучения используйте [руководство Project X с нуля](https://xtls.github.io/en/document/level-0/), [установку Xray](https://xtls.github.io/en/document/install.html) и [настройку транспорта и TLS/Reality](https://xtls.github.io/en/config/transport.html).

После настройки получите клиентскую ссылку или узел mihomo. Для Reality нужны совпадающие UUID, server name, public key и short ID; для WS/gRPC — соответствующие path/Host/service name. В Mukhomor импортируется клиентский узел, а не полный серверный JSON Xray. Сверяйте [VLESS](https://wiki.metacubex.one/en/config/proxies/vless/), [VMess](https://wiki.metacubex.one/en/config/proxies/vmess/) и [Trojan](https://wiki.metacubex.one/en/config/proxies/trojan/).

## Hysteria 2

Официальные [установка](https://v2.hysteria.network/docs/getting-started/Installation/) и [настройка сервера](https://v2.hysteria.network/docs/getting-started/Server/) описывают сервер, TLS-сертификат, пароль и запуск. Обеспечьте доступность UDP-порта. Получите клиентскую `hy2://` ссылку или [узел Hysteria 2 для mihomo](https://wiki.metacubex.one/en/config/proxies/hysteria2/), затем импортируйте его.

## Другие протоколы

| Семейство | Первичный источник для сервера | Клиент Mukhomor |
| --- | --- | --- |
| Обычный WireGuard | [Установка](https://www.wireguard.com/install/), [Quick Start](https://www.wireguard.com/quickstart/) | Один peer в `.conf` |
| Shadowsocks | [shadowsocks-rust](https://github.com/shadowsocks/shadowsocks-rust) | `ss://` или узел mihomo |
| ShadowsocksR | [shadowsocksr-backup](https://github.com/shadowsocksr-backup/shadowsocksr) | `ssr://` или узел; проверяйте состояние поддержки upstream |
| Hysteria 1 | [Hysteria](https://github.com/apernet/hysteria) | `hysteria://`; версия 1 отличается от версии 2 |
| TUIC | [TUIC](https://github.com/tuic-protocol/tuic) | `tuic://` или узел mihomo |
| Snell | [Документация сервера Snell](https://manual.nssurge.com/others/snell.html) | Узел JSON/YAML |
| AnyTLS | [anytls-go](https://github.com/anytls/anytls-go) | `anytls://` или узел |
| Mieru | [mieru](https://github.com/enfein/mieru) | Узел JSON/YAML |
| TrustTunnel | [TrustTunnel](https://github.com/TrustTunnel/TrustTunnel) | Узел JSON/YAML |
| ShadowQUIC | [ShadowQUIC](https://github.com/spongebob888/shadowquic) | Узел JSON/YAML |
| GOST Relay, HTTP, SOCKS5 | [GOST](https://gost.run/en/) | Узел / HTTP или SOCKS ссылка |
| SSH | [OpenSSH](https://www.openssh.com/manual.html) | Узел с доверенным `host-key`; TCP |
| Sudoku | [Параметры и ссылка на upstream](https://wiki.metacubex.one/en/config/proxies/sudoku/) | Узел JSON/YAML |
| MASQUE | [Параметры mihomo](https://wiki.metacubex.one/en/config/proxies/masque/) | Совместимый узел; обычный WireGuard-профиль его не заменяет |
| OpenVPN | [OpenVPN Community](https://openvpn.net/community-resources/how-to/) | Поддерживаемый inline `.ovpn` |

Это ссылки на независимые проекты, а не серверы Mukhomor и не обещание совместимости любой конфигурации. Для всех типов используйте [таблицу импорта](protocols.md).

## Что проверить на сервере

- Клиентский порт открыт в firewall VPS и в панели хостинга для правильного транспорта: TCP и UDP — разные правила.
- DNS-имя и сертификат соответствуют параметрам TLS, если протокол их использует.
- Каждый пользователь имеет отдельные реквизиты; опубликованный профиль можно отозвать.
- SSH-доступ защищён; резервная копия конфигов хранится приватно.
- Сервер получает обновления и не является открытым прокси без аутентификации.

Публичные инструкции не заменяют настройки конкретного сервера. Профили, QR-коды, ссылки подписок и ключи публиковать нельзя. Для диагностики заменяйте их учебными значениями.
