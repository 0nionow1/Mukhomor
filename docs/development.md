# Разработка

## Требования

Windows x64, Windows PowerShell 5.1, Rust stable с target `x86_64-pc-windows-msvc`, Visual Studio Build Tools с C++ и Windows SDK. Edition 2024 требует Rust 1.85 или новее; используйте актуальный stable. Node.js нужен для локальных TCP/UDP тестов, а не для клиента. Git нужен для проверки границ публикации.

## Сборка с нуля

В Windows PowerShell из корня проекта:

```powershell
.\Fetch-Dependencies.ps1
.\Build-Release.ps1 -Version 0.7.2
```

Скачивание проверяет закреплённые SHA256. Для уже загруженных зависимостей и Rust-кэша можно использовать `-Offline` у `Build-Release.ps1`. Без заранее подготовленных зависимостей этот режим не работает.

Результат в `release-staging/<версия>/windows/x64`: Windows ZIP, source ZIP и `.sha256` для каждого. Исходный репозиторий намеренно не содержит бинарники mihomo, Wintun, личные конфиги и сборочный кэш. Самостоятельный source ZIP дополнительно содержит закреплённый исходный архив ядра. Если включён Mycelium, оба ZIP содержат соответствующий архив изменённых исходников и manifest; Windows ZIP также содержит его проверенный бинарник.

Для разработки только нативной части:

```powershell
cargo test --manifest-path .\native\Cargo.toml --locked -- --test-threads=1
cargo build --manifest-path .\native\Cargo.toml --release --locked
```

Эта команда не создаёт установочный ZIP: приложение для пользователей упаковывается через `Build-Release.ps1`. Без `release-manifest.json` обычный запуск отдельного EXE может открыть ранее установленную копию. Для обновления запускайте `mukhomor.exe` из полного пакета в `release-staging`; актуальный путь записан в `release-staging/<версия>/windows/x64/latest-package.txt`.

Для проверки Slint без подключения VPN:

```powershell
cargo test --manifest-path ui/Cargo.toml --lib --locked
cargo run --manifest-path ui/Cargo.toml --features preview --locked --bin mukhomor-preview -- --render target/slint-preview
.\tests\Test-SlintStartup.ps1
```

Preview использует синтетические данные и проверяет геометрию, прокрутку, чекбоксы и состояния окон. `Test-SlintStartup` запускает настоящий EXE на отдельном тестовом каталоге, проверяет активацию второго экземпляра и штатное завершение при обновлении. Контроллер и VPN он не запускает. Обычная release-сборка включает Slint без дополнительных флагов.

Экспериментальный Mycelium — нативный исходящий адаптер в отдельной сборке mihomo. Исходники находятся в соседнем проекте `../Mycelium`; сборка закрепляет Go 1.27.2. `../Mycelium/Build-Core.ps1 -InstallIntoApp` выполняет проверки ядра и добавляет бинарник, исходный архив и manifest в приложение. Перед заменой ядра отключите работающий экземпляр приложения. Для включения приложение использует `bin/mihomo-mycelium.exe` только вместе с проверенным `bin/mycelium-build.json` и совпадающим `core_sha256` бинарника. Manifest фиксирует `schema: 1`, `protocol_version: 1`, `base_version: 1.19.32`; релизная сборка также сверяет `source_sha256` исходного архива. Обычный закреплённый mihomo остаётся базовой зависимостью. Поля профиля и правила импорта описаны в [protocols.md](protocols.md).

Для реального Mycelium профиля нужен независимый ненулевой seed длиной 32 байта, созданный CSPRNG. TLS обязательно проверяет имя/IP сертификата, срок действия и назначение для аутентификации сервера; явный SHA256 pin сохраняет эти проверки. Протокол остаётся экспериментальным. TCP/UDP и отрицательные проверки авторизации/сертификата подтверждают транспорт; устойчивость к ТСПУ требует отдельных сетевых измерений. UDP поверх TCP имеет head-of-line blocking.

## Проверки

```powershell
.\tests\Test-PublicSource.ps1
.\tests\Test-SplitVpn.ps1
.\tests\Test-BundledRouting.ps1
.\tests\Test-ProfileImport.ps1
.\tests\Test-MyceliumImport.ps1
.\tests\Test-MyceliumImport.ps1 -CorePath .\bin\mihomo-mycelium.exe
.\tests\Test-MyceliumLifecycle.ps1
.\tests\Test-ProtocolTransport.ps1
.\tests\Test-ReleasePrivacy.ps1
.\tests\Test-ReleasePrivacy.ps1 -Archive .\release-staging\0.8.2\windows\x64\Mukhomor-0.8.2-source.zip
```

Запускайте тесты отдельными PowerShell-процессами, чтобы состояние импортированных модулей не влияло на соседние проверки. Полный последовательный набор выбранных тестов: `tests/Run-Checks.ps1`; Node.js должен быть в PATH.

`Test-BundledRouting` проверяет исходные списки и их SHA-256, первый запуск с офлайн-списками, настоящие решения маршрутизации для игровых и российских доменов, границы суффиксов, приоритет процесса и переключатели пресетов. Источники в тестовом ядре заменяются файловыми провайдерами с теми же начальными данными, а VPN-выход — REJECT; все серверы находятся на loopback. Тест не меняет адаптеры, системный DNS или TUN. Тестам ACL/реестра Windows нужны права на собственные временные объекты; Rust-тесты Windows запускайте с `-- --test-threads=1` для изоляции состояния окружения.

`Test-MyceliumImport` без аргументов проверяет только чистый парсер JSON/YAML/URI, ограничения секретов, TLS и запрет внешних настроек. `-CorePath` дополнительно запускает `-t` указанного ядра в изолированной временной конфигурации. Эти проверки не запускают VPN/TUN, не меняют системный DNS и не измеряют маскировку от DPI.

`Test-MyceliumLifecycle` запускает только временные копии ядра с контроллером на loopback. Он проверяет manifest/SHA256, смену выбранного ядра, отключение при повреждённом или удалённом manifest и защиту от чужого PID/пути/времени запуска. Для этой проверки подходит стандартное ядро: если Mycelium отсутствует, используется бинарник после `Fetch-Dependencies.ps1`. Системный DNS, TUN и службы не меняются. Оба теста включены в `Run-Checks.ps1`, lifecycle остаётся и при `-SkipNative`.

После релизной сборки проверяйте оба новых архива. `Test-ReleasePrivacy.ps1` без аргументов выбирает Windows ZIP по версии в `native/Cargo.toml`; `-Archive` позволяет проверить source ZIP или явно указанный Windows ZIP. Проверка сверяет все файлы с release manifest, состав архива и отсутствие личного состояния. Source ZIP дополнительно проходит `Test-PublicSource`; отпечаток исходного архива Mycelium проверяется по вложенному manifest.

`Test-CoreRouting`, `Test-UdpRouting`, `Test-LargeRules` проверяют реальную маршрутизацию ядра на локальных серверах. `Test-NativeApp` проверяет IPC и большой список исключений. Тесты lifecycle, startup, disconnect и DNS recovery проверяют отмену и восстановление в изоляции. Проверка реального TUN, перезагрузки и конкретной игры выполняется отдельно на тестовой Windows-машине.

`tests/Measure-Routing.ps1` — локальный сравнительный замер. Его результаты не являются интернет-пингом и не измеряют Wintun или удалённый сервер.

`tests/Measure-NativeControl.ps1 -Executable .\native\target\release\mukhomor.exe -VerifyNative -Output .\native\target\control-performance.json` измеряет IPC в отдельном каталоге: переименование, автоподключение, полный и условный статус, готовность контроллера. VPN не подключается. Это не замер отрисовки или старта Slint; для сравнения контроллера используйте одинаковые release-параметры и несколько запусков.

Применение настроек возвращает предупреждения из того же расчёта правил, который использован для конфигурации ядра. Повторный расчёт не нужен; проверки ядра и откат при ошибке сохраняются. `tests/Test-NativeApp.ps1 -Executable <release-exe> -LargeRulesOnly` отдельно проверяет применение 15 000 исключений через IPC и печатает время операции.

## Как внести изменение

Смотрите [CONTRIBUTING.md](../CONTRIBUTING.md). Изменение протокола требует обновить capabilities, валидаторы, тесты, документацию и упаковку. Изменение версии согласуется с `Cargo.toml`, `Cargo.lock`, changelog и тегом. Отдельный сетевой сервис или дополнительный фреймворк для обычного изменения не нужен.
