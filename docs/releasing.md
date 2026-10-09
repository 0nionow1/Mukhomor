# Публикация релизов

## Для пользователей

Версии и скачивания находятся в [GitHub Releases](https://github.com/0nionow1/Mukhomor/releases). Для установки нужен `Mukhomor-X.Y.Z-windows-x64.zip`. Подробности: [установка и обновление](getting-started.md).

## Нумерация

Версия имеет вид `X.Y.Z`, Git-тег — `vX.Y.Z`. Пока проект в серии `0.x`, возможности и форматы могут меняться между minor версиями. Patch версия используется для исправлений; несовместимое поведение явно описывается в changelog.

Версия в `native/Cargo.toml`, запись приложения в `native/Cargo.lock`, параметр сборки и тег должны совпадать. Источник версии интерфейса — Cargo. Меняя значение по умолчанию в `Build-Release.ps1`, обновите также дефолтный путь архива в `Test-ReleasePrivacy.ps1` и примеры команд.

## Следующая версия через GitHub Actions

1. Обновите версию, [CHANGELOG.md](../CHANGELOG.md), документацию совместимости и исходники.
2. Выполните локальные проверки и отправьте изменения в `main`; дождитесь успешного CI.
3. Создайте и отправьте тег:

```powershell
git tag -a v0.5.1 -m 'Mukhomor 0.5.1'
git push origin v0.5.1
```

4. Workflow [release.yml](https://github.com/0nionow1/Mukhomor/blob/main/.github/workflows/release.yml) проверит версию, соберёт Windows и source ZIP, выполнит проверки публичных файлов и пакетов, загрузит четыре файла в **черновик** GitHub Release.
5. Откройте черновик в Releases, проверьте заметки, SHA256 и установку на тестовой машине, затем опубликуйте.

Сборка не требует личного VPN-профиля и секретов VPS. Для GitHub используется штатный краткоживущий `GITHUB_TOKEN` с правом записи contents только в release job. Не используйте реальный профиль в CI.

Релизные заметки берутся из секции `## [X.Y.Z]` changelog; секция должна существовать и быть непустой. Повторный запуск не перезаписывает существующий релиз: при ошибке изучите состояние GitHub, прежде чем повторять публикацию.

## Локальная публикация

```powershell
.\Fetch-Dependencies.ps1
.\Build-Release.ps1 -Version 0.5.0
.\tests\Run-Checks.ps1
.\tests\Test-ReleasePrivacy.ps1
.\tests\Test-ReleasePrivacy.ps1 -Archive .\dist\Mukhomor-0.5.0-source.zip
```

В GitHub создайте релиз на соответствующем теге и приложите:

- `Mukhomor-X.Y.Z-windows-x64.zip`;
- `Mukhomor-X.Y.Z-windows-x64.zip.sha256`;
- `Mukhomor-X.Y.Z-source.zip`;
- `Mukhomor-X.Y.Z-source.zip.sha256`.

Сохраните включённые лицензии и исходный архив mihomo. [Официальное описание GitHub Releases](https://docs.github.com/en/repositories/releasing-projects-on-github/about-releases).

## Границы проверки

CI и локальные тесты проверяют исходники, синтетический импорт, loopback-маршрутизацию и состав архивов. Они не подтверждают работоспособность каждого удалённого протокола, игрового античита или VPS. Перед новым публичным релизом отдельно проверьте установку, обновление, реальное подключение, отключение/восстановление DNS и сохранение профилей. Автообновление внутри клиента и подпись EXE пока не реализованы.
