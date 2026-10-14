# Протоколы и импорт

Mukhomor 0.7.2 использует **mihomo v1.19.32**; экспериментальный Mycelium требует отдельной сборки этого ядра с нативным адаптером. Таблица ниже описывает разрешённый импорт приложения, а не все функции mihomo. Источник проверки — `Get-SplitProfileCapabilities` и валидаторы в [ProfileImport.psm1](../ProfileImport.psm1); каждый импорт дополнительно проверяется закреплённым ядром.

## Поддерживаемые типы

Импорт принимает 21 стандартный тип и экспериментальный Mycelium: отдельный узел JSON или ограниченный YAML. В документации mihomo указаны параметры стандартных узлов; поддержка конкретного параметра дополнительно ограничена валидатором Mukhomor. Mycelium доступен только в сборке с его адаптером.

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
| Mycelium (экспериментальный) | `mycelium://`, узел JSON/YAML | Нативный адаптер в `mihomo-mycelium.exe` |

WireGuard/AmneziaWG — один тип исходящего узла. HTTP и HTTPS — один тип с настройкой TLS. IKEv2/IPsec, L2TP и mesh-сети Tailscale, ZeroTier, EasyTier в этой версии приложения не импортируются.

## Mycelium

Разрешены только `name`, `type`, `server`, `port`, `seed`, `sni`, `fingerprint`, `client-fingerprint`, `transport`, `udp`, `udp-transport`. `type` — `mycelium`; `seed` — индивидуальный секрет длиной 32 байта, созданный криптографическим генератором случайных чисел (CSPRNG), в виде ровно 64 шестнадцатеричных символов. Полностью нулевой seed недопустим; повторять учебный ключ для реального сервера нельзя. Порт обязателен и должен быть от 1 до 65535. `sni` задаёт имя DNS или IP для проверки сертификата; если его нет, используется адрес `server`. `udp` по умолчанию включён.

Необязательный `fingerprint` — SHA256 отпечаток самого серверного сертификата в DER: 64 шестнадцатеричных символа или 32 пары через двоеточие. В ссылке он задаётся параметром `fp`. Сертификат должен совпадать с отпечатком, соответствовать имени/IP из `sni`, быть действующим по сроку и пригодным для аутентификации TLS-сервера. Без отпечатка дополнительно проверяется цепочка доверия к центрам сертификации. Для `client-fingerprint` разрешены только `chrome`, `firefox`, `safari`, `edge`, `safari26.3`, `firefox148-compatible`; случайные профили TLS не принимаются. `skip-cert-verify`, файлы сертификатов/ключей, произвольные HTTP-заголовки и другие параметры транспорта отклоняются.

Ссылка имеет вид `mycelium://SEED@HOST:PORT?sni=IDENTITY&fp=SHA256&client-fingerprint=chrome#NAME`; `fp` и `client-fingerprint` можно опустить. Seed в ссылке должен быть записан непосредственно шестнадцатеричными символами. IPv6 адрес сервера заключают в квадратные скобки. Профиль хранит секрет, поэтому делитесь им только с владельцем соответствующего доступа.

Mycelium обрабатывает само ядро mihomo. Для включения нужны проверенные `bin/mihomo-mycelium.exe` и `bin/mycelium-build.json` с SHA256 сборки. Обычный mihomo не распознаёт `type: mycelium`. Протокол экспериментальный: устойчивость к ТСПУ/DPI не подтверждена. IP сервера, SNI, TLS fingerprint, размеры и время передачи остаются наблюдаемыми; разные seed не предотвращают блокировку IP или SNI. Импорт, локальные тесты и успешное соединение с VPS требуют дополнения измерениями на сетях российских операторов.

Для TCP JSON/YAML принимает `transport: auto`, `h2` или `websocket`; `auto` использует поддерживаемый ядром HTTP/2 с совместимым переходом на WebSocket. Ссылка URI использует выбор TCP-транспорта по умолчанию.

`udp-transport` принимает `auto`, `h3` или `websocket` в JSON/YAML и ссылке URI. Возможность HTTP/3 зависит от конкретной сборки ядра и сервера: разрешённое значение в импортере не гарантирует поддержку транспортом. Отдельный экспериментальный H3-модуль в соответствующем исходном архиве может быть только исходным прототипом. Ориентируйтесь на manifest поставляемой сборки, а не на возможности соседней рабочей ветки Mycelium.

UDP через WebSocket проходит поверх TCP: при потере пакета задерживаются и последующие данные (head-of-line blocking). Это ограничение нужно учитывать для игр и голосовой связи.

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
mycelium://0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef@vpn.example.com:443?sni=vpn.example.com&client-fingerprint=chrome#My-Mycelium
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
