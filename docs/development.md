# Разработка

## Требования

Windows x64, Windows PowerShell 5.1, Rust stable с target `x86_64-pc-windows-msvc`, Visual Studio Build Tools с C++ и Windows SDK. Edition 2024 требует Rust 1.85 или новее; используйте актуальный stable. Node.js нужен для локальных TCP/UDP тестов, а не для клиента. Git нужен для проверки границ публикации.

## Сборка с нуля

В Windows PowerShell из корня проекта:

```powershell
.\Fetch-Dependencies.ps1
.\Build-Release.ps1 -Version 0.5.0
```

Скачивание проверяет закреплённые SHA256. Для уже загруженных зависимостей и Rust-кэша можно использовать `-Offline` у `Build-Release.ps1`. Без заранее подготовленных зависимостей этот режим не работает.

Результат в `dist`: Windows ZIP, source ZIP и `.sha256` для каждого. Исходный репозиторий намеренно не содержит бинарники mihomo, Wintun, личные конфиги и сборочный кэш. Самостоятельный source ZIP дополнительно содержит закреплённый исходный архив ядра.

Для разработки только нативной части:

```powershell
cargo test --manifest-path .\native\Cargo.toml --locked
cargo build --manifest-path .\native\Cargo.toml --release --locked
```

Эта команда не создаёт установочный ZIP: приложение для пользователей упаковывается через `Build-Release.ps1`.

## Проверки

```powershell
.\tests\Test-PublicSource.ps1
.\tests\Test-SplitVpn.ps1
.\tests\Test-ProfileImport.ps1
.\tests\Test-ProtocolTransport.ps1
.\tests\Test-ReleasePrivacy.ps1
.\tests\Test-ReleasePrivacy.ps1 -Archive .\dist\Mukhomor-0.5.0-source.zip
```

Запускайте тесты отдельными PowerShell-процессами, чтобы состояние импортированных модулей не влияло на соседние проверки. Полный последовательный набор выбранных тестов: `tests/Run-Checks.ps1`; Node.js должен быть в PATH.

`Test-CoreRouting`, `Test-UdpRouting`, `Test-LargeRules` проверяют реальную маршрутизацию ядра на локальных серверах. `Test-NativeApp` проверяет IPC и большой список исключений. Тесты lifecycle, startup, disconnect и DNS recovery проверяют отмену и восстановление в изоляции. Проверка реального TUN, перезагрузки и конкретной игры выполняется отдельно на тестовой Windows-машине.

`tests/Measure-Routing.ps1` — локальный сравнительный замер. Его результаты не являются интернет-пингом и не измеряют Wintun или удалённый сервер.

## Как внести изменение

Смотрите [CONTRIBUTING.md](../CONTRIBUTING.md). Изменение протокола требует обновить capabilities, валидаторы, тесты, документацию и упаковку. Изменение версии согласуется с `Cargo.toml`, `Cargo.lock`, changelog и тегом. Отдельный сетевой сервис или дополнительный фреймворк для обычного изменения не нужен.
