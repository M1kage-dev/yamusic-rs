//! Обновления через релизы GitHub, как у оригинала: проверка раз в полчаса, тост
//! «Доступна версия …» с кнопкой «Обновить». Приложение — один exe, поэтому кнопка
//! скачивает `yamusic-rs.exe` нового релиза, сверяет SHA-256 с тем, что показывает GitHub,
//! и подменяет собой запущенный файл: Windows не даёт перезаписать работающий exe, но даёт
//! его переименовать. Старый файл (`*.old.exe`) удаляется при следующем запуске.
//! Если папка недоступна для записи, кнопка открывает страницу релиза. На Linux — всегда её.

#[cfg(windows)]
use sha2::{Digest, Sha256};
#[cfg(windows)]
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Как часто спрашивать GitHub. У оригинала UPDATE_POLL_INTERVAL_MS — те же 30 минут.
pub const POLL: Duration = Duration::from_secs(30 * 60);

/// Имя exe в релизе.
#[cfg(windows)]
const ASSET: &str = concat!(env!("CARGO_PKG_NAME"), ".exe");

/// Проверка включена в настройках («Проверять обновления»).
pub static ENABLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// Страница GitHub со всеми изменениями между запущенной версией и новой.
pub fn changes_url(version: &str) -> String {
    format!("{}/compare/v{}...v{version}", env!("CARGO_PKG_REPOSITORY"), env!("CARGO_PKG_VERSION"))
}

/// Аргумент, с которым обновлённый exe ждёт, пока закроется старый.
pub const AFTER_UPDATE: &str = "--after-update";

#[derive(Clone, Debug)]
pub struct Release {
    pub version: String,
    /// Страница релиза на GitHub.
    pub page: String,
    /// Windows: exe — адрес, размер и SHA-256. Нет — обновить можно только руками.
    #[cfg(windows)]
    pub exe: Option<(String, u64, String)>,
    /// macOS: .dmg с универсальным бандлом — адрес, размер и SHA-256.
    #[cfg(target_os = "macos")]
    pub dmg: Option<(String, u64, String)>,
}

impl Release {
    /// Можно ли обновиться из приложения: на Windows нужен exe, на macOS — dmg.
    pub fn can_install(&self) -> bool {
        #[cfg(windows)]
        return self.exe.is_some();
        #[cfg(target_os = "macos")]
        return self.dmg.is_some();
        #[cfg(not(any(windows, target_os = "macos")))]
        return false;
    }
}

/// Ассет релиза по имени: адрес, размер и SHA-256 из `digest`.
#[cfg(any(windows, target_os = "macos"))]
fn find_asset(assets: &[serde_json::Value], matches: impl Fn(&str) -> bool) -> Option<(String, u64, String)> {
    let a = assets.iter().find(|a| a.get("name").and_then(|n| n.as_str()).is_some_and(&matches))?;
    let url = a.get("browser_download_url")?.as_str()?.to_string();
    let size = a.get("size")?.as_u64()?;
    let sha = a.get("digest")?.as_str()?.strip_prefix("sha256:")?.to_ascii_lowercase();
    Some((url, size, sha))
}

pub(crate) fn http() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        // GitHub API без User-Agent отвечает 403.
        .user_agent(concat!("yamusic-rs/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())
}

/// Версия для интерфейса: «1.0.1+upstream.1.1.2» → («1.0.1», Some(«1.1.2»)).
/// Метаданные semver — не версия сборки, в UI они только мешают; базу апстрима
/// показываем отдельной подписью в «О программе».
pub fn split_version(v: &str) -> (&str, Option<&str>) {
    let v = v.trim().trim_start_matches('v');
    match v.split_once('+') {
        Some((base, meta)) => (base, meta.strip_prefix("upstream.")),
        None => (v, None),
    }
}

/// «v2.0.1» и «2.0.1+upstream.1.1.2» → (2, 0, 1). Порядок задаёт только числовая часть:
/// `+upstream.A.B.C` — это метаданные (какой апстрим внутри), они на сравнение не влияют,
/// поэтому каждый наш релиз обязан поднимать сам номер.
fn parse_version(s: &str) -> Option<(u32, u32, u32)> {
    let s = s.trim().trim_start_matches('v');
    // Хвост после «+» и после «-» (пре-релиз) на порядок не влияет.
    let base = s.split(['+', '-']).next()?;
    let mut it = base.split('.').map(|p| p.parse::<u32>());
    Some((it.next()?.ok()?, it.next()?.ok()?, it.next().unwrap_or(Ok(0)).ok()?))
}

/// Последний релиз, если он новее запущенной версии.
pub fn check() -> Result<Option<Release>, String> {
    let api = env!("CARGO_PKG_REPOSITORY").replace("https://github.com/", "https://api.github.com/repos/");
    let resp = http()?.get(format!("{api}/releases/latest")).send().map_err(|e| crate::config::chain(&e))?;
    // 404 — релизов ещё нет.
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !resp.status().is_success() {
        return Err(format!("GitHub: HTTP {}", resp.status()));
    }
    let v: serde_json::Value = resp.json().map_err(|e| e.to_string())?;
    let tag = v.get("tag_name").and_then(|t| t.as_str()).ok_or("в релизе нет версии")?;
    let (Some(latest), Some(current)) = (parse_version(tag), parse_version(env!("CARGO_PKG_VERSION"))) else {
        return Err(format!("непонятная версия {tag}"));
    };
    if latest <= current {
        return Ok(None);
    }
    // На Linux обновляемся со страницы релиза: пакет ставит система.
    #[cfg(any(windows, target_os = "macos"))]
    let assets = v.get("assets").and_then(|a| a.as_array());
    #[cfg(windows)]
    let exe = assets.and_then(|assets| find_asset(assets, |n| n == ASSET));
    #[cfg(target_os = "macos")]
    let dmg = assets.and_then(|assets| find_asset(assets, |n| n.starts_with("yamusic-rs-") && n.ends_with("-macos.dmg")));
    Ok(Some(Release {
        version: tag.trim_start_matches('v').to_string(),
        // Страницу откроет браузер системы: адрес из ответа берём, только если это https.
        page: v
            .get("html_url")
            .and_then(|u| u.as_str())
            .and_then(crate::client::browser_url)
            .unwrap_or_else(|| env!("CARGO_PKG_REPOSITORY").to_string()),
        #[cfg(windows)]
        exe,
        #[cfg(target_os = "macos")]
        dmg,
    }))
}

#[cfg(windows)]
fn old_path(exe: &Path) -> PathBuf {
    exe.with_extension("old.exe")
}

/// Поставить обновление и перезапуститься. Windows подменяет exe, macOS — бандл `.app`
/// (см. `update_macos`). Приложение закрывается сразу после успеха.
pub fn apply(r: &Release) -> Result<(), String> {
    #[cfg(windows)]
    {
        let exe = install_exe(r)?;
        relaunch(&exe)
    }
    #[cfg(target_os = "macos")]
    {
        crate::update_macos::install(r)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = r;
        Err("обновление вручную: пакет ставит система".into())
    }
}

/// Скачать новый exe, проверить и поставить на место запущенного. Долго: звать из фонового
/// потока. После успеха нужно запустить [`relaunch`] и закрыться.
#[cfg(windows)]
fn install_exe(r: &Release) -> Result<PathBuf, String> {
    let (url, size, sha) = r.exe.as_ref().ok_or("у релиза нет exe")?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let bytes = http()?
        .get(url)
        .timeout(Duration::from_secs(600))
        .send()
        .and_then(|r| r.error_for_status())
        .and_then(|r| r.bytes())
        .map_err(|e| format!("загрузка: {}", crate::config::chain(&e)))?;
    if bytes.len() as u64 != *size {
        return Err("файл скачался не целиком".into());
    }
    let got: String = Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect();
    if &got != sha {
        return Err("контрольная сумма не совпала".into());
    }
    let new = exe.with_extension("new.exe");
    std::fs::write(&new, &bytes).map_err(|e| format!("нет доступа к папке {}: {e}", exe.parent().unwrap_or(&exe).display()))?;
    let old = old_path(&exe);
    let _ = std::fs::remove_file(&old);
    if let Err(e) = std::fs::rename(&exe, &old) {
        let _ = std::fs::remove_file(&new);
        return Err(format!("не удалось заменить exe: {e}"));
    }
    if let Err(e) = std::fs::rename(&new, &exe) {
        let _ = std::fs::rename(&old, &exe);
        return Err(format!("не удалось заменить exe: {e}"));
    }
    Ok(exe)
}

/// Запустить обновлённый exe. Он дождётся, пока этот процесс закроется.
#[cfg(windows)]
fn relaunch(exe: &Path) -> Result<(), String> {
    std::process::Command::new(exe)
        .arg(AFTER_UPDATE)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("новая версия не запустилась: {e}"))
}

/// Убрать exe прошлой версии, оставшийся после обновления. Старый процесс может ещё
/// завершаться и держать файл, поэтому пробуем несколько раз в фоне. На macOS прошлый
/// бандл убирает скрипт замены, здесь делать нечего.
pub fn cleanup() {
    #[cfg(windows)]
    {
        let Ok(exe) = std::env::current_exe() else { return };
        let old = old_path(&exe);
        if !old.exists() {
            return;
        }
        std::thread::spawn(move || {
            for _ in 0..10 {
                if std::fs::remove_file(&old).is_ok() || !old.exists() {
                    return;
                }
                std::thread::sleep(Duration::from_secs(2));
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(parse_version("v0.2.0"), Some((0, 2, 0)));
        assert_eq!(parse_version("1.10"), Some((1, 10, 0)));
        assert!(parse_version("v0.10.0") > parse_version("0.9.9"));
        assert_eq!(parse_version("latest"), None);
    }

    /// Метаданные `+upstream.A.B.C` не участвуют в сравнении: порядок задаёт номер.
    #[test]
    fn upstream_metadata_is_ignored() {
        assert_eq!(parse_version("2.0.0+upstream.1.1.2"), Some((2, 0, 0)));
        assert_eq!(parse_version("v2.0.1+upstream.1.2.0"), Some((2, 0, 1)));
        assert!(parse_version("2.0.1+upstream.1.1.2") > parse_version("2.0.0+upstream.1.1.2"));
        assert_eq!(parse_version("2.0.0+upstream.1.1.2"), parse_version("2.0.0"));
    }

    /// В интерфейсе — только номер сборки, база апстрима отдельно.
    #[test]
    fn version_for_ui() {
        assert_eq!(split_version("1.0.1+upstream.1.1.2"), ("1.0.1", Some("1.1.2")));
        assert_eq!(split_version("v2.0.0"), ("2.0.0", None));
        assert_eq!(split_version("1.1.2+other.3"), ("1.1.2", None));
    }

    #[cfg(windows)]
    #[test]
    fn paths() {
        assert_eq!(old_path(Path::new(r"C:\a\yamusic-rs.exe")), Path::new(r"C:\a\yamusic-rs.old.exe"));
        assert_eq!(ASSET, "yamusic-rs.exe");
    }
}
