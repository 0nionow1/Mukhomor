# Публикация релизов

## Для пользователей

Версии и скачивания находятся в [GitHub Releases](https://github.com/0nionow1/Mukhomor/releases). Для установки выберите Windows ZIP, Linux tar.gz или Android APK. Подробности: [установка и обновление](getting-started.md).

## Нумерация

Версия имеет вид `X.Y.Z`, Git-тег — `vX.Y.Z`. Пока проект в серии `0.x`, возможности и форматы могут меняться между minor версиями. Patch версия используется для исправлений; несовместимое поведение явно описывается в changelog.

Версия в `native/Cargo.toml`, запись приложения в `native/Cargo.lock`, параметр сборки и тег должны совпадать. Источник версии интерфейса — Cargo. Скрипты сборки и проверки архивов читают версию из Cargo автоматически.

## Следующая версия через GitHub Actions

Агенты поддерживают историю изменений по [AGENTS.md](../AGENTS.md). До выпуска
пользовательские изменения записываются в `CHANGELOG.md` → `[Unreleased]` и
`website/changelog.json` → `unreleased`. После подтверждённой публикации Release
перенесите краткие заметки в `releases`, укажите версию и фактический
`published_at` из GitHub, затем выполните `node website/scripts/changelog.mjs`,
`node website/scripts/seo.mjs` и `node website/scripts/check.mjs`. Дата на сайте отображается по Москве.
Не включайте локальные промежуточные сборки в список опубликованных версий.
После разрешённой владельцем публикации обновляйте сайт вместе с релизом. Импортируйте файлы и `published_at` из GitHub API в `website/content.json`, сохраните предыдущие версии и выполните генераторы content, changelog, seo и check. Не приписывайте новому импорту статус Stable без проверки владельцем.

1. Обновите версию, [CHANGELOG.md](../CHANGELOG.md), документацию совместимости и исходники.
2. Выполните локальные проверки и отправьте изменения в `main`; дождитесь успешного CI.
3. Создайте и отправьте тег:

```powershell
git tag -a v0.8.1 -m 'Mukhomor 0.8.1'
git push origin v0.8.1
```

4. Workflow [release.yml](https://github.com/0nionow1/Mukhomor/blob/main/.github/workflows/release.yml) проверит версию, соберёт Windows ZIP, Linux tar.gz, подписанный Android APK и соответствующие исходники, выполнит проверки публичных файлов и пакетов, загрузит десять файлов в **черновик** GitHub Release.
5. Откройте черновик в Releases, проверьте заметки, SHA256 и установку на тестовой машине, затем опубликуйте.

Стандартная сборка Actions использует закреплённый mihomo. Проверенные локальные артефакты Mycelium не входят в Git: для выпуска с этим ядром собирайте пакеты локально и прикладывайте оба архива вместе с SHA256 и соответствующими исходниками. Не заменяйте их стандартной сборкой без проверки состава.

Сборка не требует личного VPN-профиля и секретов VPS. Для GitHub используется штатный краткоживущий `GITHUB_TOKEN` с правом записи contents только в release job. Не используйте реальный профиль в CI.

Релизные заметки берутся из секции `## [X.Y.Z]` changelog; секция должна существовать и быть непустой. Повторный запуск не перезаписывает существующий релиз: при ошибке изучите состояние GitHub, прежде чем повторять публикацию.

## Локальная публикация

```powershell
.\Fetch-Dependencies.ps1
.\Build-Release.ps1 -Version 0.8.1
.\tests\Run-Checks.ps1
.\tests\Test-ReleasePrivacy.ps1
.\tests\Test-ReleasePrivacy.ps1 -Archive .\release-staging\0.8.1\windows\x64\Mukhomor-0.8.1-source.zip
```

В GitHub создайте релиз на соответствующем теге и приложите:

- `Mukhomor-X.Y.Z-windows-x64.zip`;
- `Mukhomor-X.Y.Z-windows-x64.zip.sha256`;
- `Mukhomor-X.Y.Z-source.zip`;
- `Mukhomor-X.Y.Z-source.zip.sha256`;
- `Mukhomor-X.Y.Z-linux-x64.tar.gz` и его `.sha256` (исходники Linux внутри);
- `Mukhomor-X.Y.Z-android.apk` и его `.sha256`;
- `Mukhomor-X.Y.Z-android-source.zip` и его `.sha256`.

Для Android используйте постоянный ключ подписи. В Actions нужны зашифрованные секреты `MUKHOMOR_ANDROID_KEYSTORE_BASE64`, `MUKHOMOR_KEYSTORE_PASSWORD` и `MUKHOMOR_ANDROID_KEY_ALIAS`. Значения не должны попадать в Git, аргументы команд или логи. Debug APK не заменяет подписанную выпускную сборку.

Сохраните включённые лицензии и исходный архив mihomo. [Официальное описание GitHub Releases](https://docs.github.com/en/repositories/releasing-projects-on-github/about-releases).

## Материалы GitHub при смене дизайна

Обновляйте существующие изображения вместе с новыми: `assets/github-banner.svg`, `assets/preview.png`, `assets/appearance-preview.png` и графики должны соответствовать текущему интерфейсу. Основной визуальный ориентир — текущий дизайн сайта. Сначала получите актуальные Slint-фикстуры: `cargo run --manifest-path ui/Cargo.toml --features preview --locked --bin mukhomor-preview -- --render target/public-release-preview`, затем в Windows выполните `website/scripts/export-github-assets.ps1`. Проверяйте не только README, но и превью ссылок репозитория.

Готовая картинка для превью — `assets/github-social-preview.png` (1280 × 640). Её нужно отдельно загрузить в настройках Social preview репозитория: изменение файла в Git само по себе не меняет выбранное изображение GitHub. Перед публикацией проверяйте все изображения на тестовых данных, без рабочих профилей и серверных реквизитов.

## Границы проверки

CI и локальные тесты проверяют исходники, синтетический импорт, loopback-маршрутизацию и состав архивов. Они не подтверждают работоспособность каждого удалённого протокола, игрового античита или VPS. Перед новым публичным релизом отдельно проверьте установку, обновление, реальное подключение, отключение/восстановление DNS и сохранение профилей. Автообновление внутри клиента и подпись EXE пока не реализованы.
