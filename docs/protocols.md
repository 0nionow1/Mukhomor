# Протоколы и импорт

Mukhomor 0.5.0 использует **mihomo v1.19.32**. Таблица ниже описывает разрешённый импорт приложения, а не все функции mihomo. Источник проверки — `Get-SplitProfileCapabilities` и валидаторы в [ProfileImport.psm1](../ProfileImport.psm1); каждый импорт дополнительно проверяется закреплённым ядром.

## Поддерживаемые типы

Все 21 типа принимают отдельный узел JSON или ограниченный YAML. В документации mihomo указаны параметры узлов; поддержка конкретного параметра дополнительно ограничена валидатором Mukhomor.

| Тип | Ссылка / специальный файл | Параметры mihomo |
| --- | --- | --- |
| WireGuard / AmneziaWG | `.conf`, один peer | [WireGuard](https://wiki.metacubex.one/en/config/proxies/wg/) |
| VLESS | `vless://` | [VLESS](https://wiki.metacubex.one/en/config/proxies/vless/) |
| VMess | `vmess://` | [VMess](https://wiki.metacubex.one/en/config/proxies/vmess/) |
| Trojan | `trojan://` | [Trojan](https://wiki.metacubex.one/en/config/proxies/trojan/) |
| Shadowsocks | `ss://` | [Shadowsocks](https://wiki.metacubex.one/en/config/proxies/ss/) |
| ShadowsocksR | `ssr://` | [ShadowsocksR](https://wiki.metacubex.one/en/config/proxies/ssr/) |
| Hysteria | `hysteria://` | [Hysteria](https://wiki.metacubex.one/en/config/proxies/hysteria/) |
| Hysteria 2 | `hy2://`, `hysteria2://` | [Hysteria 2](https://wiki.metacubex.one/en/config/proxies/hysteria2/) |
| TUIC | `tuic://` | [TUIC](https://wiki.metacubex.one/en/config/proxies/tuic/) |
| Snell | Узел JSON/YAML | [Snell](https://wiki.metacubex.one/en/config/proxies/snell/) |
| SOCKS5 | `socks5://`, `socks5s://` | [SOCKS](https://wiki.metacubex.one/en/config/proxies/socks/) |
| HTTP / HTTPS | `http://`, `https://` с адресом прокси | [HTTP](https://wiki.metacubex.one/en/config/proxies/http/) |
| SSH | Узел JSON/YAML с доверенным `host-key` | [SSH](https://wiki.metacubex.one/en/config/proxies/ssh/) |
| AnyTLS | `anytls://` | [AnyTLS](https://wiki.metacubex.one/en/config/proxies/anytls/) |
| Mieru | Узел JSON/YAML | [Mieru](https://wiki.metacubex.one/en/config/proxies/mieru/) |
| TrustTunnel | Узел JSON/YAML | [TrustTunnel](https://wiki.metacubex.one/en/config/proxies/trusttunnel/) |
| ShadowQUIC | Узел JSON/YAML | [ShadowQUIC](https://wiki.metacubex.one/en/config/proxies/shadowquic/) |
| GOST Relay | Узел JSON/YAML, `type: gost-relay` | [GOST](https://gost.run/en/tutorials/protocols/relay/) |
| Sudoku | Узел JSON/YAML | [Sudoku](https://wiki.metacubex.one/en/config/proxies/sudoku/) |
| MASQUE | Узел JSON/YAML | [MASQUE](https://wiki.metacubex.one/en/config/proxies/masque/) |
| OpenVPN | Ограниченный inline `.ovpn` | [OpenVPN](https://wiki.metacubex.one/en/config/proxies/openvpn/) |

WireGuard/AmneziaWG — один тип исходящего узла. HTTP и HTTPS — один тип с настройкой TLS. IKEv2/IPsec, L2TP и mesh-сети Tailscale, ZeroTier, EasyTier в этой версии приложения не импортируются.

## Форматы и границы

- UTF-8 текст до **512 КиБ**; до **128 серверов** за операцию и **512** в библиотеке.
- Название: 1–80 печатных символов.
- Ссылка или несколько ссылок, по одной на строку; base64 содержимое списка ссылок.
- JSON: отдельный узел, массив узлов либо объект только с полем `proxies`.
- YAML: те же узлы, отступы по два пробела, без тегов, якорей, merge и произвольных глобальных секций.
- `.conf`: один WireGuard/AmneziaWG peer, IPv4 туннеля и необязательный IPv6. Скрипты `PostUp`/`PostDown` отклоняются. Формат Amnezia `.vpn` не равнозначен `.conf`.
- `.ovpn`: один `remote`, `dev tun`, сертификаты/ключи и при необходимости `auth-user-pass` внутри файла. Неподдерживаемые директивы, включая `verify-x509-name`, отклоняются.

Не импортируются глобальные настройки Clash, DNS/TUN, провайдеры подписок, группы/цепочки прокси, внешние файлы ключей или скрипты. Валидация сначала проверяет всю пачку; ошибка не должна менять библиотеку или выбранный профиль. После успешного добавления прежний выбранный сервер остаётся выбранным.

## TLS, Reality и транспорты

Адрес сервера, SNI, WS Host и Reality public key — разные параметры. Копируйте их из выданного профиля без подмены одним значением. Поддерживаются разрешённые параметры TLS/Reality, WS, gRPC и других транспортов соответствующего типа; неизвестный параметр приводит к ошибке импорта. Проверка `mihomo -t` проверяет конфигурацию, а не доступность удалённого сервера.

SSH требует доверенного ключа хоста. Для OpenVPN профиль с внешними файлами необходимо корректно экспортировать как inline; удалять проверки сертификата ради импорта не следует.

## TCP и UDP

HTTP/HTTPS и SSH передают TCP. UDP других типов зависит от сервера, параметров узла и транспорта. Наличие протокола в списке не означает, что любой его сервер передаст UDP.

Прямое исключение игры применяется к TCP и UDP независимо от возможностей VPN-сервера. Для не исключённых игр и голосовой связи проверяйте UDP отдельно.

## Учебные примеры

Ниже нет работающего сервера или настоящих ключей. Замените адрес, UUID/пароль и SNI данными своего сервера.

```text
vless://00000000-0000-4000-8000-000000000000@vpn.example.com:443?security=tls&sni=vpn.example.com&type=ws&host=vpn.example.com&path=%2Fvpn#My-server
hy2://REPLACE_WITH_SERVER_PASSWORD@vpn.example.com:443?sni=vpn.example.com#My-Hysteria
```

Минимальный отдельный узел Shadowsocks:

```json
{
  "name": "My Shadowsocks",
  "type": "ss",
  "server": "vpn.example.com",
  "port": 8388,
  "cipher": "aes-128-gcm",
  "password": "REPLACE_WITH_SERVER_PASSWORD",
  "udp": true
}
```

Серверный конфиг Xray или Hysteria нельзя вставить вместо клиентского узла mihomo. Как получить подходящий профиль: [свой VPS и VPN](vps-and-vpn.md).
