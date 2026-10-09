use serde_json::json;
use std::{
    borrow::Cow,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
use windows_sys::Win32::{
    Foundation::{
        ERROR_ACCESS_DENIED, ERROR_LOCK_VIOLATION, ERROR_SHARING_VIOLATION, GetLastError,
    },
    Globalization::GetUserDefaultUILanguage,
    Storage::FileSystem::{MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW},
    UI::Shell::CSIDL_LOCAL_APPDATA,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Language {
    Auto,
    Russian,
    English,
}

impl Language {
    fn value(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Russian => "ru",
            Self::English => "en",
        }
    }
}

pub struct Locale {
    preference: Language,
    system: Language,
    path: PathBuf,
}

impl Locale {
    pub fn new(root: &Path, smoke: bool) -> Result<Self, String> {
        let directory = if smoke {
            root.join("runtime")
        } else {
            crate::setup::folder(CSIDL_LOCAL_APPDATA)?.join("Mukhomor")
        };
        let path = directory.join("ui-preferences.json");
        let preference = std::fs::File::open(&path)
            .ok()
            .and_then(|file| {
                let mut bytes = Vec::new();
                file.take(4097).read_to_end(&mut bytes).ok()?;
                Some(bytes)
            })
            .filter(|bytes| bytes.len() <= 4096)
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .map(|value| match value["language"].as_str() {
                Some("ru") => Language::Russian,
                Some("en") => Language::English,
                _ => Language::Auto,
            })
            .unwrap_or(Language::Auto);
        Ok(Self {
            preference,
            system: system_language(unsafe { GetUserDefaultUILanguage() }),
            path,
        })
    }

    pub fn language(&self) -> Language {
        self.preference
    }

    fn effective(&self) -> Language {
        if self.preference == Language::Auto {
            self.system
        } else {
            self.preference
        }
    }

    pub fn set_language(&mut self, language: Language) -> Result<(), String> {
        if self.preference == language {
            return Ok(());
        }
        static SERIAL: AtomicU64 = AtomicU64::new(0);
        let parent = self.path.parent().ok_or("No preference directory")?;
        std::fs::create_dir_all(parent).map_err(|_| "Не удалось сохранить язык интерфейса")?;
        let temporary = parent.join(format!(
            "ui-preferences-{}-{}.tmp",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        let result: Result<(), String> = (|| {
            let mut output = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
                .map_err(|_| "Не удалось сохранить язык интерфейса")?;
            output
                .write_all(json!({"language":language.value()}).to_string().as_bytes())
                .and_then(|_| output.sync_all())
                .map_err(|_| "Не удалось сохранить язык интерфейса")?;
            drop(output);
            replace_preference(&temporary, &self.path)?;
            Ok(())
        })();
        if temporary.exists() {
            let _ = std::fs::remove_file(&temporary);
        }
        result?;
        self.preference = language;
        Ok(())
    }

    pub fn text<'a>(&self, source: &'a str) -> Cow<'a, str> {
        if self.effective() != Language::English {
            return Cow::Borrowed(source);
        }
        if let Some((_, english)) = TEXT.iter().find(|(russian, _)| *russian == source) {
            return Cow::Borrowed(english);
        }
        if let Some(count) = source.strip_prefix("ТВОИ СЕРВЕРЫ · ") {
            return Cow::Owned(format!("YOUR SERVERS · {count}"));
        }
        if let Some(count) = source.strip_prefix("СЕРВЕРЫ · ") {
            return Cow::Owned(format!("SERVERS · {count}"));
        }
        if let Some(count) = source.strip_prefix("Добавлено: ") {
            return Cow::Owned(format!("Added: {count}"));
        }
        if let Some(message) = source.strip_suffix(" · проверяем состояние") {
            return Cow::Owned(format!("{} · checking status", self.text(message)));
        }
        Cow::Borrowed(source)
    }

    /// Translate presentation only. The original errors and IPC payloads stay intact.
    pub fn error<'a>(&self, source: &'a str) -> Cow<'a, str> {
        if self.effective() != Language::English {
            if let Some(reason) = source.strip_prefix("Profile import: ")
                && let Some(translated) = import_error_russian(reason)
            {
                return Cow::Borrowed(translated);
            }
            match source {
                "Import supports up to 128 servers and a library of up to 512 servers" => {
                    return Cow::Borrowed(
                        "Импорт: не более 128 серверов за один раз и 512 серверов в приложении.",
                    );
                }
                "Invalid profile library" | "Invalid profile library entry" => {
                    return Cow::Borrowed("Список серверов повреждён. Проверь локальные журналы.");
                }
                "Selected server is missing from the profile library" => {
                    return Cow::Borrowed(
                        "Выбранный сервер отсутствует в списке. Выбери другой сервер.",
                    );
                }
                _ => {}
            }
            if source == "Import and select a server first" {
                return Cow::Borrowed("Сначала добавь конфигурацию и выбери сервер.");
            }
            if let Some(phase) = source
                .strip_prefix("VPN connection check failed (")
                .and_then(|rest| {
                    rest.strip_suffix("). Startup will be rolled back; see runtime/logs/core.log.")
                })
            {
                return Cow::Owned(format!(
                    "Проверка VPN не удалась ({phase}). Подключение отменено; подробности в runtime/logs/core.log."
                ));
            }
            return Cow::Borrowed(source);
        }
        if let Some(message) = source.strip_suffix(" · проверяем состояние") {
            return Cow::Owned(format!("{} · checking status", self.error(message)));
        }
        let translated = self.text(source);
        if translated != source {
            return translated;
        }
        if let Some((action, rest)) = source
            .strip_prefix("Операция ")
            .and_then(|rest| rest.split_once(" превысила "))
            && let Some(seconds) =
                rest.strip_suffix(" секунд. Подключение завершено с ошибкой; проверь диагностику.")
            && !seconds.is_empty()
            && seconds.bytes().all(|byte| byte.is_ascii_digit())
        {
            let action = match action {
                "Connect" => "Connection",
                "Select" => "Server switch",
                "Disconnect" => "Disconnection",
                "Exit" => "Shutdown",
                "Init" => "Startup",
                "Snapshot" => "Status check",
                "Import" => "Profile import",
                "Rename" => "Server rename",
                "Remove" => "Server removal",
                "ApplySettings" => "Saving settings",
                "Autoconnect" => "Saving startup preferences",
                "Update" => "List update",
                "UpdateDns" => "DNS update",
                "Diagnose" => "Diagnostics",
                _ => "The operation",
            };
            return Cow::Owned(format!(
                "{action} timed out after {seconds} seconds. Check Diagnostics."
            ));
        }
        // An unfamiliar Windows/backend error is evidence, even if Windows
        // returned it in another language. Never replace it with a generic
        // message or infer a different cause from a single matching word.
        Cow::Borrowed(source)
    }
}

fn retryable_replace_error(code: u32) -> bool {
    matches!(
        code,
        ERROR_ACCESS_DENIED | ERROR_LOCK_VIOLATION | ERROR_SHARING_VIOLATION
    )
}

fn replace_preference(temporary: &Path, destination: &Path) -> Result<(), String> {
    let source = crate::platform::wide(&temporary.to_string_lossy());
    let target = crate::platform::wide(&destination.to_string_lossy());
    // A file scanner can briefly open the previous preference without delete
    // sharing. Reuse the already closed temporary file; never retry indefinitely.
    const DELAYS_MS: [u64; 5] = [10, 20, 40, 40, 40];
    for delay in DELAYS_MS.into_iter().map(Some).chain(std::iter::once(None)) {
        if unsafe {
            MoveFileExW(
                source.as_ptr(),
                target.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        } != 0
        {
            return Ok(());
        }
        let error = unsafe { GetLastError() };
        if !retryable_replace_error(error) {
            break;
        }
        let Some(delay) = delay else {
            break;
        };
        std::thread::sleep(std::time::Duration::from_millis(delay));
    }
    Err("Не удалось сохранить язык интерфейса".into())
}

fn system_language(language_id: u16) -> Language {
    if language_id & 0x03ff == 0x0019 {
        Language::Russian
    } else {
        Language::English
    }
}

const TEXT: &[(&str, &str)] = &[
    (
        "Приложение содержит недопустимые знаки или имеет недопустимую длину.",
        "The application path contains invalid characters or exceeds the Windows length limit.",
    ),
    ("Подробности ошибки", "Error details"),
    ("Выбери и подключись", "Select & connect"),
    (
        "Не удалось загрузить фактуру интерфейса Mukhomor.",
        "Could not load Mukhomor interface textures.",
    ),
    (
        "Не удалось найти системную папку Windows",
        "Could not locate a Windows system folder",
    ),
    (
        "Нет доступа к данным Mukhomor",
        "Cannot access Mukhomor data",
    ),
    (
        "Эта установка Mukhomor принадлежит другому пользователю Windows",
        "This Mukhomor installation belongs to another Windows user",
    ),
    (
        "Установка Mukhomor ещё не завершена",
        "Mukhomor installation is not complete yet",
    ),
    (
        "Повреждены сведения об установке",
        "Installation information is damaged",
    ),
    ("Нет пути приложения", "Application path is missing"),
    (
        "Файлы Mukhomor не найдены",
        "Mukhomor files could not be found",
    ),
    (
        "Папка установки не найдена",
        "Installation folder could not be found",
    ),
    (
        "Недопустимый путь установленного приложения",
        "Invalid installed application path",
    ),
    (
        "Установка отменена. При первом запуске Windows запрашивает разрешение на настройку VPN.",
        "Installation cancelled. Windows asks for permission to set up the VPN on first launch.",
    ),
    ("Не удалось запустить Mukhomor", "Could not start Mukhomor"),
    (
        "Настройка Mukhomor не завершилась вовремя",
        "Mukhomor setup timed out",
    ),
    (
        "Не удалось настроить Mukhomor. Проверь права администратора и целостность архива.",
        "Could not set up Mukhomor. Check administrator permissions and archive integrity.",
    ),
    (
        "Запусти mukhomor.exe из распакованного релиза. Для разработки используй режим --smoke.",
        "Run mukhomor.exe from the extracted release. For development, use --smoke.",
    ),
    (
        "Не удалось открыть контроллер Mukhomor",
        "Could not open the Mukhomor controller",
    ),
    (
        "Не удалось открыть Mukhomor. Запусти обновлённый архив для восстановления установки.",
        "Could not open Mukhomor. Run the updated extracted release to repair the installation.",
    ),
    (
        "Не удалось проверить установленный контроллер",
        "Could not verify the installed controller",
    ),
    (
        "Не указан путь установленного контроллера",
        "Installed controller path is missing",
    ),
    (
        "Не удалось проверить состояние Mukhomor",
        "Could not check Mukhomor status",
    ),
    (
        "Не удалось запустить Mukhomor. Запусти обновлённый архив для восстановления установки.",
        "Could not start Mukhomor. Run the updated extracted release to repair the installation.",
    ),
    (
        "Mukhomor не запустился вовремя. Повтори запуск приложения.",
        "Mukhomor startup timed out. Try opening the application again.",
    ),
    (
        "Не удалось проверить контроллер Mukhomor",
        "Could not verify the Mukhomor controller",
    ),
    (
        "Не удалось получить состояние Mukhomor",
        "Could not read Mukhomor status",
    ),
    ("Некорректный пользователь Windows", "Invalid Windows user"),
    (
        "Не удалось загрузить встроенный шрифт Mukhomor.",
        "Could not load Mukhomor's embedded font.",
    ),
    (
        "Не удалось проверить установленный контроллер Mukhomor",
        "Unable to verify the installed Mukhomor controller.",
    ),
    (
        "Не удалось прочитать сведения о контроллере Mukhomor",
        "Unable to read Mukhomor controller information.",
    ),
    (
        "Контроллер не принадлежит этой установке Mukhomor",
        "The controller does not belong to this Mukhomor installation.",
    ),
    (
        "Не удалось проверить состояние контроллера Mukhomor",
        "Unable to check the Mukhomor controller status.",
    ),
    (
        "Не удалось создать ожидание IPC",
        "Unable to wait for a response from Mukhomor.",
    ),
    (
        "Соединение с Mukhomor прервано",
        "The connection to Mukhomor was interrupted.",
    ),
    (
        "Mukhomor не ответил вовремя. Состояние подключения проверяется отдельно.",
        "Mukhomor did not respond in time. Connection status is being checked separately.",
    ),
    (
        "Mukhomor ещё запускается. Подожди несколько секунд.",
        "Mukhomor is starting. Please wait a few seconds.",
    ),
    (
        "Не удалось прочитать состояние подключения; автоматическое подключение отменено",
        "Unable to read the saved connection state. Automatic connection was cancelled.",
    ),
    (
        "Повреждено состояние подключения; автоматическое подключение отменено",
        "The saved connection state is damaged. Automatic connection was cancelled.",
    ),
    (
        "Неизвестный формат состояния подключения; автоматическое подключение отменено",
        "The saved connection state format is unsupported. Automatic connection was cancelled.",
    ),
    (
        "Повреждено состояние подключения",
        "The saved connection state is damaged.",
    ),
    (
        "Не удалось сохранить состояние подключения. Проверь доступ к данным Mukhomor.",
        "Unable to save the connection state. Check access to the Mukhomor data folder.",
    ),
    ("Неизвестная операция", "Unknown operation."),
    (
        "Дождись завершения текущей операции или отключи VPN.",
        "Wait for the current operation to finish, or disconnect the VPN.",
    ),
    (
        "Не удалось записать сигнал отмены. Отключение выполняется; проверь доступ к данным Mukhomor.",
        "Unable to save the cancellation request. Disconnection is in progress; check access to the Mukhomor data folder.",
    ),
    (
        "Операция превысила время ожидания",
        "The operation timed out.",
    ),
    (
        "Не удалось остановить процессы Mukhomor",
        "Unable to stop Mukhomor background processes.",
    ),
    (
        "Не удалось дождаться фоновой операции Mukhomor",
        "Unable to wait for the Mukhomor background operation.",
    ),
    (
        "Фоновый процесс не закрыл ответ; операция остановлена",
        "The background process did not finish its response. The operation was stopped.",
    ),
    (
        "Фоновая операция не вернула корректный ответ",
        "The background operation returned an invalid response.",
    ),
    (
        "Операция не выполнена",
        "The operation could not be completed.",
    ),
    ("Подключение", "Connection"),
    ("Исключения", "Bypass rules"),
    ("DNS и списки", "DNS & lists"),
    ("Настройки", "Settings"),
    ("Имена .exe", "App names"),
    ("Пути .exe", "App paths"),
    ("Домены", "Domains"),
    ("Поддомены · TLD", "Subdomains · TLD"),
    ("Через VPN", "Use VPN"),
    ("Устанавливаем соединение…", "Connecting…"),
    ("Выбираем сервер…", "Selecting server…"),
    ("Добавляем конфигурацию…", "Importing configuration…"),
    ("Отключаем VPN…", "Disconnecting VPN…"),
    (
        "Отключаем VPN и завершаем Mukhomor…",
        "Disconnecting and exiting Mukhomor…",
    ),
    ("Проверяем соединение…", "Checking connection…"),
    ("Сохраняем изменения…", "Saving changes…"),
    (
        "Интервалы и лимиты DNS должны быть целыми положительными числами",
        "DNS intervals and limits must be positive whole numbers",
    ),
    ("Проверь настройки DNS", "Check DNS settings"),
    ("Сервер", "Server"),
    ("Отменить", "Cancel"),
    ("Отключиться", "Disconnect"),
    ("Отключаем…", "Disconnecting…"),
    ("Вернуть сеть", "Restore network"),
    ("Подключиться", "Connect"),
    ("Операция отменена", "Operation cancelled"),
    ("Подключение отменено", "Connection cancelled"),
    (
        "Не удалось завершить операцию",
        "The operation could not be completed",
    ),
    ("МУХАМОР", "MUKHOMOR"),
    (
        "Трафик по этим правилам идёт напрямую",
        "Matching traffic bypasses the VPN",
    ),
    (
        "Российские сервисы и актуальные адреса",
        "Russian services and current addresses",
    ),
    ("Твой VPN. Твои привычки.", "Your VPN. Your preferences."),
    ("VPN подключён", "VPN connected"),
    ("Отключение", "Disconnecting"),
    ("Восстановление сети", "Network recovery"),
    ("VPN отключён", "VPN disconnected"),
    (
        "Исключения работают · TCP + UDP",
        "Bypass rules active · TCP + UDP",
    ),
    (
        "Подключение можно отменить",
        "You can cancel the connection",
    ),
    (
        "Восстанавливаем обычное подключение",
        "Restoring your normal connection",
    ),
    (
        "Нажми, чтобы восстановить обычное подключение",
        "Click to restore your normal connection",
    ),
    ("Выбери сервер и подключись", "Select a server and connect"),
    ("Первый сервер ждёт тебя", "Add your first server"),
    (
        "Добавь конфигурацию AmneziaWG .conf",
        "Import an AmneziaWG .conf file",
    ),
    ("Имя сервера", "Server name"),
    (
        "cs2.exe, qbittorrent.exe — по одному имени на строку. TCP и UDP программы идут напрямую.",
        "cs2.exe, qbittorrent.exe — one name per line. The app's TCP and UDP traffic bypasses the VPN.",
    ),
    (
        "Полный путь отличает конкретную программу. Выбери установленный .exe или добавь путь вручную.",
        "Use a full path to identify a specific app. Choose an installed .exe or enter its path.",
    ),
    (
        "api.example.com — только точное имя. Без https://, порта и пути страницы.",
        "api.example.com — an exact hostname. Leave out https://, ports and page paths.",
    ),
    (
        "example.com — домен с поддоменами. ru — вся зона .ru. рф — вся зона .рф.",
        "example.com includes subdomains. ru matches all .ru domains; рф matches all .рф domains.",
    ),
    (
        "IPv4, IPv6 и подсети: 1.2.3.4, 1.2.3.0/24, 2001:db8::/32.",
        "IPv4, IPv6 and subnets: 1.2.3.4, 1.2.3.0/24, 2001:db8::/32.",
    ),
    (
        "Эти домены пойдут через VPN, даже если присутствуют в готовом списке сервисов.",
        "These domains use the VPN even when included in a preset service list.",
    ),
    ("ГОТОВЫЕ ИСКЛЮЧЕНИЯ", "PRESET BYPASS LISTS"),
    (
        "Домены и поддомены обновляются автоматически.\nOzon также входит в российский список.",
        "Domains and subdomains update automatically.\nOzon is also included in the Russian list.",
    ),
    ("IP ПО ДОМЕНАМ", "DOMAIN IP ADDRESSES"),
    (
        "Доменные правила работают и без IP-кэша.",
        "Domain rules also work without an IP cache.",
    ),
    ("Домены для обновления IP", "Domains for IP updates"),
    ("Мин. интервал, сек", "Min. interval, sec"),
    ("Макс. интервал, сек", "Max. interval, sec"),
    ("При ошибке, сек", "Keep stale, sec"),
    ("Лимит доменов", "Domain limit"),
    ("Адресов на домен", "IPs per domain"),
    (
        "Учитывается TTL ответа. При сбое остаются последние\nпроверенные адреса в пределах заданного срока.",
        "Updates respect DNS TTL. If a request fails, the last\nverified addresses remain valid for the configured period.",
    ),
    ("Сразу после запуска Windows", "When Windows starts"),
    (
        "Подключится последний выбранный сервер.\nМожно отключить VPN в любой момент.",
        "Connect to the last selected server.\nYou can disconnect at any time.",
    ),
    ("ЛЕГКО В ФОНЕ", "LIGHTWEIGHT IN THE BACKGROUND"),
    (
        "Можно закрыть окно — подключение продолжит работать.\nВ трее интерфейс ждёт без фоновой перерисовки.",
        "Close the window to keep your connection active.\nThe tray interface waits without background rendering.",
    ),
    ("ЕСЛИ ЧТО-ТО ПОШЛО НЕ ТАК", "NEED HELP?"),
    (
        "Проверь связь и открой локальные журналы.",
        "Check your connection and open local logs.",
    ),
    (
        "Ключи конфигураций хранятся только на этом компьютере.\nНикакой регистрации и загрузки профилей в облако.",
        "Configuration keys stay on this computer.\nNo accounts and no profile uploads to the cloud.",
    ),
    ("Приложения Windows", "Windows applications"),
    ("Конфигурация AmneziaWG", "AmneziaWG configuration"),
    ("Mukhomor · открыть настройки", "Mukhomor · open settings"),
    ("AmneziaWG · выбран", "AmneziaWG · selected"),
    (
        "Нет подтверждения состояния VPN. Нажми «Вернуть сеть» для повторного отключения.",
        "VPN status could not be confirmed. Click Restore network to try disconnecting again.",
    ),
    ("Сервер добавлен", "Server added"),
    ("Изменения сохранены", "Changes saved"),
    ("Списки и IP обновлены", "Lists and IP addresses updated"),
    (
        "Проверка завершена. Результаты в журналах.",
        "Diagnostics completed. Results are in the logs.",
    ),
    ("Не удалось выполнить действие", "The operation failed"),
    (
        "Нет ответа от Mukhomor. Повторяем проверку состояния…",
        "No response from Mukhomor. Checking status again…",
    ),
    (
        "Файл должен иметь кодировку UTF-8",
        "The file must use UTF-8 encoding",
    ),
    (
        "Не удалось прочитать .conf (максимум 512 КБ)",
        "Could not read the .conf file (maximum 512 KB)",
    ),
    ("Переименовать", "Rename"),
    ("Удалить профиль", "Remove profile"),
    (
        "Удалить выбранный профиль с этого компьютера?",
        "Remove the selected profile from this computer?",
    ),
    ("Удаление сервера", "Remove server"),
    (
        "Сначала добавь свой файл AmneziaWG .conf и выбери сервер.",
        "Import your AmneziaWG .conf file and select a server first.",
    ),
    ("Нужен сервер", "No server selected"),
    (
        "Интервалы DNS должны быть целыми числами в секундах",
        "DNS intervals must be whole numbers of seconds",
    ),
    ("Проверь интервалы", "Check intervals"),
    ("Открыть настройки", "Open settings"),
    ("Отключить VPN", "Disconnect VPN"),
    ("Выход из Mukhomor", "Exit Mukhomor"),
    (
        "Не удалось открыть окно Mukhomor",
        "Could not open the Mukhomor window",
    ),
    (
        "Окно Mukhomor уже открывается. Попробуй ещё раз через несколько секунд.",
        "Mukhomor is already opening. Try again in a few seconds.",
    ),
    (
        "Не удалось создать окно приложения",
        "Could not create the application window",
    ),
    ("Свернуть окно", "Minimize window"),
    ("Закрыть окно", "Close window"),
    ("Прокрутка", "Scroll"),
    ("+ Добавить конфигурацию", "+ Add configuration"),
    ("Добавить конфигурацию", "Add configuration"),
    ("Из файла…", "From file…"),
    ("Вставить ссылку или текст…", "Paste a link or text…"),
    ("Новая конфигурация", "New configuration"),
    ("Конфигурации VPN", "VPN configurations"),
    ("Файл конфигурации пуст", "The configuration file is empty"),
    ("Импортировать", "Import"),
    ("Добавляем…", "Importing…"),
    ("Отмена", "Cancel"),
    ("Конфигурация добавлена", "Configuration added"),
    (
        "Вставь ссылку или текст конфигурации (Ctrl+V)",
        "Paste a link or configuration text (Ctrl+V)",
    ),
    (
        "Добавь файл конфигурации или ссылку",
        "Add a configuration file or a link",
    ),
    (
        "Сначала добавь конфигурацию и выбери сервер.",
        "Add a configuration and select a server first.",
    ),
    (
        "Конфигурация слишком большая (максимум 512 КБ)",
        "The configuration is too large (maximum 512 KB)",
    ),
    (
        "Не удалось прочитать конфигурацию (максимум 512 КБ)",
        "Could not read the configuration (maximum 512 KB)",
    ),
    ("Выбрать", "Select"),
    ("ОК", "OK"),
    ("Удалить", "Remove"),
    ("Подключить VPN", "Connect VPN"),
    ("Добавить .exe", "Add .exe"),
    ("Сохранить", "Save"),
    ("Обновить списки и IP", "Update lists & IPs"),
    ("Сохранить настройки DNS", "Save DNS settings"),
    ("Диагностика", "Diagnostics"),
    ("Открыть журналы", "Open logs"),
    ("Свернуть в трей", "Minimize to tray"),
    ("Российские сервисы", "Russian services"),
    ("Обновлять IP доменов", "Update domain IPs"),
    (
        "Учитывать замеченные поддомены",
        "Include observed subdomains",
    ),
    ("Автоподключение", "Auto-connect"),
    ("ЯЗЫК ИНТЕРФЕЙСА", "INTERFACE LANGUAGE"),
    ("Язык интерфейса", "Interface language"),
    ("По языку Windows", "Use Windows language"),
    ("Системный", "System"),
    ("Автоматически", "Automatic"),
    ("Авто", "Auto"),
    ("Русский", "Русский"),
    (
        "Не удалось сохранить язык интерфейса",
        "Could not save the interface language",
    ),
];

fn import_error_russian(reason: &str) -> Option<&'static str> {
    Some(match reason {
        "empty input or size limit exceeded." => "Импорт: текст пуст или превышает 512 КБ.",
        "invalid input or size limit exceeded." => {
            "Импорт: текст содержит недопустимые символы или превышает 512 КБ."
        }
        "invalid Unicode." | "invalid Unicode option." => "Импорт: неверная кодировка Unicode.",
        "unsupported input format." => {
            "Импорт: формат не поддерживается. Выбери конфигурацию или вставь ссылку на сервер."
        }
        "malformed input or incompatible protocol options." => {
            "Импорт: повреждённая конфигурация или несовместимые параметры протокола."
        }
        "node count exceeds 128." | "node count must be between 1 and 128." => {
            "Импорт: в одной конфигурации должно быть от 1 до 128 серверов."
        }
        "server name must contain 1 to 80 printable characters." => {
            "Импорт: название сервера должно содержать от 1 до 80 печатных символов."
        }
        "unsupported protocol." => "Импорт: этот протокол не поддерживается.",
        "unsupported node option or configuration directive." => {
            "Импорт: параметр сервера или директива конфигурации не поддерживается."
        }
        "expected one outgoing node." => "Импорт: ожидалась конфигурация одного сервера.",
        "expected outgoing node objects." => {
            "Импорт: каждый сервер должен быть объектом конфигурации."
        }
        "only a standalone proxies collection is accepted; global settings are unsupported." => {
            "Импорт: разрешён только список proxies; общие настройки Mihomo не импортируются."
        }
        "invalid normalized profile envelope." => {
            "Импорт: неверная структура сохранённой конфигурации сервера."
        }
        "server is missing." => "Импорт: адрес сервера не указан.",
        "invalid server hostname." => "Импорт: неверное имя или IP-адрес сервера.",
        "endpoint port must be between 1 and 65535." => {
            "Импорт: порт сервера должен быть от 1 до 65535."
        }
        "unspecified or scoped endpoint is unsupported." => {
            "Импорт: неопределённый адрес или IPv6 с идентификатором зоны не поддерживается."
        }
        "a required protocol credential or option is missing." => {
            "Импорт: не хватает обязательного ключа, учётных данных или параметра протокола."
        }
        "credential or identity option has an invalid type." => {
            "Импорт: неверный тип учётных данных или идентификатора сервера."
        }
        "invalid UUID." => "Импорт: неверный UUID пользователя.",
        "invalid node option." | "unsupported option value." => {
            "Импорт: неверное или неподдерживаемое значение параметра сервера."
        }
        "invalid option key." => "Импорт: неверное имя параметра.",
        "invalid boolean option." | "boolean node option has an invalid type." => {
            "Импорт: логический параметр должен иметь значение true или false."
        }
        "node option exceeds its limit." => "Импорт: параметр сервера превышает допустимый размер.",
        "numeric option exceeds its limit." | "invalid numeric resource limit." => {
            "Импорт: числовой параметр выходит за допустимые границы."
        }
        "option list exceeds its limit." => "Импорт: список параметров слишком большой.",
        "option object exceeds its limit." => "Импорт: объект параметров слишком большой.",
        "concurrency exceeds its limit." => {
            "Импорт: превышено допустимое число одновременных соединений."
        }
        "worker count exceeds its limit." => {
            "Импорт: число рабочих потоков не должно превышать 32."
        }
        "normalized profile exceeds its storage limit." => {
            "Импорт: конфигурация сервера превышает допустимый размер для хранения."
        }
        "receive window exceeds its limit." => "Импорт: размер окна приёма слишком большой.",
        "invalid TLS server identity." => "Импорт: неверное имя сервера TLS.",
        "unsupported nested options." => {
            "Импорт: вложенные параметры этого типа не поддерживаются."
        }
        "invalid transport options." | "unsupported transport option." => {
            "Импорт: неверные или неподдерживаемые параметры транспорта."
        }
        "invalid header option." | "invalid header options." => {
            "Импорт: неверные параметры заголовков."
        }
        "unsupported built-in plugin." => "Импорт: встроенный плагин не поддерживается.",
        "invalid plugin options." | "unsupported plugin option." => {
            "Импорт: неверные или неподдерживаемые параметры плагина."
        }
        "duplicate plugin option." => "Импорт: параметр плагина указан несколько раз.",
        "invalid base64 data." => "Импорт: неверные данные base64.",
        "invalid base64 or UTF-8 data." => "Импорт: неверные данные base64 или кодировка UTF-8.",
        "base64 subscription must contain share links." => {
            "Импорт: закодированный список base64 должен содержать ссылки на серверы."
        }
        "invalid URL encoding." | "invalid URL encoding or UTF-8." => {
            "Импорт: неверная кодировка ссылки."
        }
        "invalid share link." => "Импорт: повреждённая ссылка на сервер.",
        "invalid share-link endpoint." => "Импорт: в ссылке неверно указан адрес или порт сервера.",
        "unsupported share-link scheme." => "Импорт: протокол ссылки не поддерживается.",
        "unexpected share-link path." => "Импорт: ссылка содержит неподдерживаемый путь.",
        "unsupported or duplicate link parameter." => {
            "Импорт: параметр ссылки не поддерживается или указан несколько раз."
        }
        "unsupported link security mode." => {
            "Импорт: режим защиты соединения в ссылке не поддерживается."
        }
        "unsupported link transport." => "Импорт: транспорт в ссылке не поддерживается.",
        "HTTP and SOCKS share links do not accept global parameters." => {
            "Импорт: ссылки HTTP и SOCKS не могут содержать общие параметры конфигурации."
        }
        "incomplete VMess link." => "Импорт: в ссылке VMess не хватает обязательных параметров.",
        "invalid VMess link." => "Импорт: повреждённая ссылка VMess.",
        "unsupported VMess link option." => "Импорт: параметр ссылки VMess не поддерживается.",
        "unsupported VMess security." => "Импорт: режим шифрования VMess не поддерживается.",
        "unsupported VMess header camouflage." => {
            "Импорт: маскировка заголовков VMess не поддерживается."
        }
        "invalid Shadowsocks credentials." => "Импорт: неверные учётные данные Shadowsocks.",
        "invalid ShadowsocksR link." => "Импорт: повреждённая ссылка ShadowsocksR.",
        "Reality parameters require Reality security." => {
            "Импорт: параметры Reality требуют выбранного режима защиты Reality."
        }
        "Reality public key is missing." => "Импорт: публичный ключ Reality не указан.",
        "TUIC authentication is missing." => "Импорт: не указаны учётные данные TUIC.",
        "SSH authentication is missing." => "Импорт: не указаны учётные данные SSH.",
        "SSH requires explicit server host keys." => {
            "Импорт: для SSH необходимо явно указать ключи сервера."
        }
        "Snell versions 1 and 2 do not support UDP." => {
            "Импорт: Snell версий 1 и 2 не поддерживает UDP."
        }
        "MASQUE h3-l4proxy does not support UDP." => {
            "Импорт: MASQUE h3-l4proxy не поддерживает UDP."
        }
        "expected Interface and one Peer." => {
            "Импорт: конфигурация WG должна содержать Interface и один Peer."
        }
        "only one WireGuard peer is supported." => {
            "Импорт: поддерживается только один Peer WireGuard."
        }
        "WireGuard credentials or endpoint are missing." => {
            "Импорт: не указаны ключи или адрес сервера WireGuard."
        }
        "required AmneziaWG field is missing." => {
            "Импорт: не указан обязательный параметр AmneziaWG."
        }
        "duplicate WireGuard field." => "Импорт: параметр WireGuard указан несколько раз.",
        "unsupported WireGuard or AmneziaWG field." => {
            "Импорт: параметр WireGuard или AmneziaWG не поддерживается."
        }
        "unsupported WireGuard peer field." => "Импорт: параметр Peer WireGuard не поддерживается.",
        "invalid WireGuard profile line." => "Импорт: неверная строка конфигурации WireGuard.",
        "invalid WireGuard endpoint." => "Импорт: неверный адрес сервера WireGuard.",
        "invalid WireGuard key." => "Импорт: неверный ключ WireGuard.",
        "invalid tunnel address." => "Импорт: неверный адрес туннеля.",
        "IPv4 tunnel address is required." => "Импорт: необходимо указать IPv4-адрес туннеля.",
        "invalid tunnel MTU." => "Импорт: неверное значение MTU туннеля.",
        "invalid, duplicate or excessively nested JSON." => {
            "Импорт: повреждённый JSON, повторяющиеся поля или слишком большая глубина вложенности."
        }
        "invalid or excessively large YAML." => "Импорт: повреждённый или слишком большой YAML.",
        "duplicate YAML option." => "Импорт: параметр YAML указан несколько раз.",
        "empty YAML item." => "Импорт: в YAML обнаружен пустой элемент.",
        "empty YAML option." => "Импорт: в YAML обнаружен пустой параметр.",
        "invalid YAML quoted value." => "Импорт: неверное значение YAML в кавычках.",
        "unsupported YAML mapping syntax." => {
            "Импорт: этот синтаксис объектов YAML не поддерживается."
        }
        "mixed YAML containers are unsupported." => {
            "Импорт: смешивание списков и объектов YAML не поддерживается."
        }
        "mixed YAML roots are unsupported." => {
            "Импорт: в YAML должен быть один корневой объект или список."
        }
        "YAML aliases, tags, blocks and non-JSON flow syntax are unsupported." => {
            "Импорт: ссылки, теги, блоки и встроенные конструкции YAML вне синтаксиса JSON не поддерживаются."
        }
        "YAML documents and merge directives are unsupported." => {
            "Импорт: несколько документов YAML и директивы слияния не поддерживаются."
        }
        "YAML indentation must use two-space levels." => {
            "Импорт: каждый уровень отступа YAML должен состоять из двух пробелов."
        }
        "YAML tabs are unsupported." => "Импорт: замени табуляцию в YAML пробелами.",
        "YAML integer exceeds its limit." => "Импорт: число в YAML выходит за допустимые границы.",
        "YAML list exceeds its limit." => "Импорт: список YAML слишком большой.",
        "YAML object exceeds its limit." => "Импорт: объект YAML слишком большой.",
        "YAML nesting exceeds its limit." => {
            "Импорт: превышена допустимая глубина вложенности YAML."
        }
        "keys and certificates must be inline PEM; file paths are unsupported." => {
            "Импорт: ключи и сертификаты PEM должны быть внутри конфигурации; пути к внешним файлам не поддерживаются."
        }
        "only OpenVPN TUN mode is supported." => "Импорт: поддерживается только режим TUN OpenVPN.",
        "OpenVPN certificates and keys must be inline." => {
            "Импорт: сертификаты и ключи OpenVPN должны быть внутри конфигурации."
        }
        "OpenVPN requires exactly one explicit remote and port." => {
            "Импорт: OpenVPN должен содержать одну директиву remote с адресом и портом."
        }
        "OpenVPN remote or inline authentication is missing." => {
            "Импорт: не указан адрес сервера или встроенные учётные данные OpenVPN."
        }
        "external OpenVPN authentication files are unsupported." => {
            "Импорт: внешние файлы авторизации OpenVPN не поддерживаются; вставь учётные данные в конфигурацию."
        }
        "inline OpenVPN authentication requires username and password." => {
            "Импорт: встроенная авторизация OpenVPN требует логина и пароля."
        }
        "unsupported OpenVPN directive, file reference or script." => {
            "Импорт: директива, внешний файл или скрипт OpenVPN не поддерживается."
        }
        "unsupported OpenVPN certificate role." => {
            "Импорт: роль сертификата OpenVPN не поддерживается."
        }
        "invalid OpenVPN client directive." | "invalid OpenVPN directive." => {
            "Импорт: неверная директива OpenVPN."
        }
        "invalid OpenVPN numeric directive." => {
            "Импорт: неверное числовое значение директивы OpenVPN."
        }
        "invalid OpenVPN verbosity." => "Импорт: неверный уровень журналирования OpenVPN.",
        "duplicate OpenVPN option." => "Импорт: параметр OpenVPN указан несколько раз.",
        "duplicate OpenVPN inline option." => {
            "Импорт: встроенный блок OpenVPN указан несколько раз."
        }
        "unterminated OpenVPN inline option." => "Импорт: встроенный блок OpenVPN не закрыт.",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_language_selection() {
        assert_eq!(system_language(0x0419), Language::Russian);
        assert_eq!(system_language(0x0819), Language::Russian);
        for id in [0x0409, 0x0809, 0x0407, 0x0411, 0x0000] {
            assert_eq!(system_language(id), Language::English);
        }
    }

    #[test]
    fn manual_preference_and_dynamic_translation() {
        let mut locale = Locale {
            preference: Language::Auto,
            system: Language::English,
            path: PathBuf::new(),
        };
        assert_eq!(locale.text("Подключиться"), "Connect");
        assert_eq!(locale.text("ТВОИ СЕРВЕРЫ · 12"), "YOUR SERVERS · 12");
        locale.preference = Language::Russian;
        assert_eq!(locale.text("Подключиться"), "Подключиться");
        locale.system = Language::Russian;
        locale.preference = Language::English;
        assert_eq!(locale.text("Настройки"), "Settings");
        assert_eq!(locale.text("api.example.com"), "api.example.com");
    }

    #[test]
    fn every_public_parser_error_has_a_russian_display_without_changing_english() {
        let mut locale = Locale {
            preference: Language::Russian,
            system: Language::English,
            path: PathBuf::new(),
        };
        let module = include_str!("../../ProfileImport.psm1");
        let mut messages = std::collections::HashSet::new();
        for suffix in module.split("'Profile import: ").skip(1) {
            let reason = suffix.split('\'').next().unwrap();
            let source = format!("Profile import: {reason}");
            let translated = locale.error(&source);
            assert!(
                translated.starts_with("Импорт: "),
                "Missing Russian translation: {source}"
            );
            assert_ne!(translated, source);
            locale.preference = Language::English;
            assert_eq!(locale.error(&source), source);
            locale.preference = Language::Russian;
            messages.insert(reason);
        }
        assert!(
            messages.len() >= 100,
            "The public parser fixture must cover its full error catalog"
        );
        assert_eq!(
            locale.error("Profile import: a future diagnostic."),
            "Profile import: a future diagnostic."
        );
    }

    #[test]
    fn generic_import_and_connection_errors_translate_without_changing_payload() {
        let mut locale = Locale {
            preference: Language::English,
            system: Language::Russian,
            path: PathBuf::new(),
        };
        assert_eq!(locale.text("Добавлено: 12"), "Added: 12");
        assert_eq!(locale.text("Импортировать"), "Import");
        assert_eq!(
            locale.text("Вставить ссылку или текст…"),
            "Paste a link or text…"
        );
        let source = "VPN connection check failed (after rules). Startup will be rolled back; see runtime/logs/core.log.";
        assert_eq!(locale.error(source), source);
        locale.preference = Language::Russian;
        assert_eq!(
            locale.error("Import and select a server first"),
            "Сначала добавь конфигурацию и выбери сервер."
        );
        assert_eq!(
            locale.error(source),
            "Проверка VPN не удалась (after rules). Подключение отменено; подробности в runtime/logs/core.log."
        );
        assert_eq!(
            locale.error("Unknown protocol option: fixture.example"),
            "Unknown protocol option: fixture.example"
        );
    }

    #[test]
    fn runtime_errors_are_display_only_and_keep_english_details() {
        let locale = Locale {
            preference: Language::English,
            system: Language::Russian,
            path: PathBuf::new(),
        };
        let raw = "Подключение отменено";
        assert_eq!(locale.error(raw), "Connection cancelled");
        assert_eq!(raw, "Подключение отменено");
        assert_eq!(
            locale.error("Операция Exit превысила 20 секунд. Подключение завершено с ошибкой; проверь диагностику."),
            "Shutdown timed out after 20 seconds. Check Diagnostics."
        );
        assert_eq!(
            locale.error("Отказано в доступе (os error 5) · проверяем состояние"),
            "Отказано в доступе (os error 5) · checking status"
        );
        assert_eq!(
            locale.error("Invalid domain: пример"),
            "Invalid domain: пример"
        );
        assert_eq!(
            locale.error("Unknown Russian error: ошибка"),
            "Unknown Russian error: ошибка"
        );
        assert_eq!(
            locale.error("Приложение содержит недопустимые знаки или имеет недопустимую длину."),
            "The application path contains invalid characters or exceeds the Windows length limit."
        );
        assert_eq!(locale.error("Сервер добавлен"), "Server added");
    }

    #[test]
    fn unknown_backend_errors_keep_the_actual_cause_in_both_languages() {
        let mut locale = Locale {
            preference: Language::English,
            system: Language::Russian,
            path: PathBuf::new(),
        };
        let errors = [
            "Синтетическая неизвестная ошибка Windows · fixture.example",
            "Отказано в доступе к adapter42: нет прав на создание правила (0x80070005)",
            "Unknown Russian error: ошибка · 0x80070057",
        ];
        for language in [Language::English, Language::Russian] {
            locale.preference = language;
            for source in errors {
                assert_eq!(locale.error(source), source);
            }
        }
    }

    #[test]
    fn preference_survives_restart_and_corrupt_input_is_safe() {
        let root = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!(
                "locale-test-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
        let result = || {
            let mut locale = Locale::new(&root, true).unwrap();
            assert_eq!(locale.language(), Language::Auto);
            locale.set_language(Language::English).unwrap();
            assert_eq!(
                Locale::new(&root, true).unwrap().language(),
                Language::English
            );
            locale.set_language(Language::Russian).unwrap();
            assert_eq!(
                Locale::new(&root, true).unwrap().language(),
                Language::Russian
            );
            locale.set_language(Language::Auto).unwrap();
            assert_eq!(Locale::new(&root, true).unwrap().language(), Language::Auto);
            std::fs::write(&locale.path, b"broken JSON").unwrap();
            assert_eq!(Locale::new(&root, true).unwrap().language(), Language::Auto);
            std::fs::write(&locale.path, vec![b' '; 4097]).unwrap();
            assert_eq!(Locale::new(&root, true).unwrap().language(), Language::Auto);
            let files: Vec<_> = std::fs::read_dir(root.join("runtime")).unwrap().collect();
            assert_eq!(files.len(), 1);
        };
        let tested = std::panic::catch_unwind(result);
        let _ = std::fs::remove_dir_all(&root);
        tested.unwrap();
    }

    #[test]
    fn preference_replacement_handles_transient_lock_and_bounds_permanent_failure() {
        use windows_sys::Win32::{
            Foundation::{CloseHandle, INVALID_HANDLE_VALUE},
            Storage::FileSystem::{
                CreateFileW, FILE_GENERIC_READ, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
            },
        };
        let root = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!(
                "locale-lock-test-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
        let tested = std::panic::catch_unwind(|| {
            let mut locale = Locale::new(&root, true).unwrap();
            locale.set_language(Language::English).unwrap();
            let lock = || {
                let handle = unsafe {
                    CreateFileW(
                        crate::platform::wide(&locale.path.to_string_lossy()).as_ptr(),
                        FILE_GENERIC_READ,
                        FILE_SHARE_READ | FILE_SHARE_WRITE,
                        std::ptr::null(),
                        OPEN_EXISTING,
                        0,
                        std::ptr::null_mut(),
                    )
                };
                assert_ne!(handle, INVALID_HANDLE_VALUE);
                handle
            };
            let handle = lock() as isize;
            let release = std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(45));
                unsafe { CloseHandle(handle as _) };
            });
            let switched = locale.set_language(Language::Russian);
            release.join().unwrap();
            switched.unwrap();
            assert_eq!(
                Locale::new(&root, true).unwrap().language(),
                Language::Russian
            );
            let handle = unsafe {
                CreateFileW(
                    crate::platform::wide(&locale.path.to_string_lossy()).as_ptr(),
                    FILE_GENERIC_READ,
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    std::ptr::null(),
                    OPEN_EXISTING,
                    0,
                    std::ptr::null_mut(),
                )
            };
            assert_ne!(handle, INVALID_HANDLE_VALUE);
            let started = std::time::Instant::now();
            let failed = locale.set_language(Language::English);
            unsafe { CloseHandle(handle) };
            assert!(failed.is_err());
            assert!(started.elapsed() < std::time::Duration::from_secs(2));
            assert_eq!(locale.language(), Language::Russian);
            assert_eq!(
                Locale::new(&root, true).unwrap().language(),
                Language::Russian
            );
            assert_eq!(std::fs::read_dir(root.join("runtime")).unwrap().count(), 1);
            assert!(retryable_replace_error(5));
            assert!(retryable_replace_error(32));
            assert!(retryable_replace_error(33));
            assert!(!retryable_replace_error(2));
            assert!(!retryable_replace_error(3));
            assert!(replace_preference(&root.join("missing.tmp"), &locale.path).is_err());
        });
        let _ = std::fs::remove_dir_all(&root);
        tested.unwrap();
    }
}
