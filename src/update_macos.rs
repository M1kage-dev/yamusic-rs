//! Автообновление на macOS: скачать `.dmg` из релиза, сверить SHA-256, смонтировать,
//! проверить подпись нового бандла и подменить `.app` после нашего выхода.
//!
//! Подменить работающий бандл нельзя: macOS держит файлы открытыми. Поэтому установка
//! сводится к тому, чтобы подготовить новый `.app` и запустить маленький скрипт, который
//! дождётся завершения процесса, переставит бандл и откроет новую версию.

use crate::update::Release;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

/// Скрипт замены. Все пути — аргументы: пробелы, кириллица и кавычки в них ничего не ломают.
/// `$1` — pid, `$2` — бандл на месте, `$3` — новый бандл, `$4` — временная папка,
/// `$5` — пауза перед уборкой старого бандла (в тестах 0).
const SWAP: &str = r#"#!/bin/sh
while kill -0 "$1" 2>/dev/null; do sleep 0.3; done
sleep 0.5
rm -rf "$2.old"
if ! mv "$2" "$2.old"; then open "$2" 2>/dev/null; exit 1; fi
if ! cp -R "$3" "$2"; then rm -rf "$2"; mv "$2.old" "$2"; open "$2" 2>/dev/null; exit 1; fi
xattr -dr com.apple.quarantine "$2" 2>/dev/null
open "$2" 2>/dev/null
sleep "${5:-3}"
rm -rf "$2.old"
rm -rf "$4"
"#;

/// Скачать и поставить обновление. Возвращается сразу после запуска замены: приложение
/// должно закрыться, иначе скрипт будет ждать вечно.
pub fn install(r: &Release) -> Result<(), String> {
    let app = crate::sys::bundle_path().ok_or("приложение запущено не из .app — обновите вручную")?;
    let parent = app.parent().ok_or("у .app нет папки")?;
    if !writable(parent) {
        return Err(format!("нет прав на запись в {}", parent.display()));
    }
    let (staged, tmp) = prepare(r)?;
    if bundle_id(&staged)? != bundle_id(&app)? {
        return Err("в образе другое приложение".into());
    }
    let script = tmp.join("swap.sh");
    std::fs::write(&script, SWAP).map_err(|e| format!("скрипт замены: {e}"))?;
    Command::new("/bin/sh")
        .arg(&script)
        .arg(std::process::id().to_string())
        .arg(&app)
        .arg(&staged)
        .arg(&tmp)
        .arg("3")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("замена не запустилась: {e}"))?;
    Ok(())
}

/// Скачать образ и подготовить новый бандл: сверить SHA-256, смонтировать, скопировать
/// и проверить подпись. Ничего установленного не трогает — этим занимается [`install`].
pub fn prepare(r: &Release) -> Result<(PathBuf, PathBuf), String> {
    let (url, size, sha) = r.dmg.as_ref().ok_or("у релиза нет .dmg")?;
    let tmp = crate::config::cache_dir().join("update");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(|e| format!("временная папка: {e}"))?;

    let dmg = tmp.join("update.dmg");
    let bytes = crate::update::http()?
        .get(url)
        .timeout(Duration::from_secs(1800))
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
    std::fs::write(&dmg, &bytes).map_err(|e| format!("запись образа: {e}"))?;

    let mnt = tmp.join("mnt");
    std::fs::create_dir_all(&mnt).map_err(|e| format!("точка монтирования: {e}"))?;
    attach(&dmg, &mnt)?;
    let staged = tmp.join("new.app");
    let result = (|| {
        copy_dir(&first_app(&mnt)?, &staged)?;
        signed(&staged)
    })();
    detach(&mnt);
    result?;
    Ok((staged, tmp))
}

/// Можно ли писать рядом с бандлом: проверяем пробным файлом, а не правами на глаз.
fn writable(dir: &Path) -> bool {
    let probe = dir.join(".yamusic-rs-update-probe");
    match std::fs::OpenOptions::new().create_new(true).write(true).open(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

fn bundle_id(app: &Path) -> Result<String, String> {
    let plist = app.join("Contents").join("Info.plist");
    let out = Command::new("/usr/bin/plutil")
        .args(["-extract", "CFBundleIdentifier", "raw"])
        .arg(&plist)
        .output()
        .map_err(|e| format!("plutil: {e}"))?;
    if !out.status.success() {
        return Err("в бандле нет CFBundleIdentifier".into());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Первый `.app` в смонтированном образе.
fn first_app(mnt: &Path) -> Result<PathBuf, String> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(mnt).map_err(|e| format!("образ: {e}"))?.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "app") && path.is_dir() {
            found.push(path);
        }
    }
    match found.len() {
        1 => Ok(found.remove(0)),
        0 => Err("в образе нет .app".into()),
        _ => Err("в образе несколько .app".into()),
    }
}

fn attach(dmg: &Path, mnt: &Path) -> Result<(), String> {
    let out = Command::new("/usr/bin/hdiutil")
        .args(["attach", "-nobrowse", "-readonly", "-noautoopen", "-mountpoint"])
        .arg(mnt)
        .arg(dmg)
        .output()
        .map_err(|e| format!("hdiutil attach: {e}"))?;
    if !out.status.success() {
        return Err(format!("образ не смонтировался: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(())
}

/// Отмонтировать. Образ может быть ещё занят проверкой подписи, поэтому пара попыток.
fn detach(mnt: &Path) {
    for _ in 0..5 {
        let ok = Command::new("/usr/bin/hdiutil")
            .arg("detach")
            .arg(mnt)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        if ok {
            return;
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}

/// Подпись обязана проверяться: ставить неподписанный или битый бандл нельзя.
fn signed(app: &Path) -> Result<(), String> {
    let ok = Command::new("/usr/bin/codesign")
        .args(["--verify", "--deep", "--strict"])
        .arg(app)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if ok {
        Ok(())
    } else {
        Err("подпись нового бандла не проверяется".into())
    }
}

/// Рекурсивная копия: `std::fs::copy` не умеет папки, а внутри `.app` есть симлинки.
fn copy_dir(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|e| format!("копия {}: {e}", to.display()))?;
    for entry in std::fs::read_dir(from).map_err(|e| format!("чтение {}: {e}", from.display()))?.flatten() {
        let target = to.join(entry.file_name());
        let ty = entry.file_type().map_err(|e| format!("тип {}: {e}", entry.path().display()))?;
        if ty.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else if ty.is_symlink() {
            let link = std::fs::read_link(entry.path()).map_err(|e| format!("симлинк: {e}"))?;
            std::os::unix::fs::symlink(link, &target).map_err(|e| format!("симлинк {}: {e}", target.display()))?;
        } else {
            std::fs::copy(entry.path(), &target).map_err(|e| format!("копия {}: {e}", target.display()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dir(PathBuf);

    impl Dir {
        fn new(tag: &str) -> Dir {
            let p = std::env::temp_dir().join(format!("ym-update-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Dir(p)
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Скрипт замены — самая опасная часть: проверяем её на настоящих папках.
    #[test]
    fn swap_script_replaces_the_bundle() {
        let dir = Dir::new("swap");
        // Бандл лежит отдельно от временной папки: скрипт убирает $4 целиком.
        let apps = dir.0.join("Applications");
        let tmp = dir.0.join("tmp");
        std::fs::create_dir_all(&apps).unwrap();
        std::fs::create_dir_all(&tmp).unwrap();
        let old = apps.join("App.app");
        let new = tmp.join("new.app");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::create_dir_all(&new).unwrap();
        std::fs::write(old.join("version.txt"), "old").unwrap();
        std::fs::write(new.join("version.txt"), "new").unwrap();

        // 999999 — заведомо несуществующий pid: ждать нечего.
        let ok = Command::new("/bin/sh")
            .arg("-c")
            .arg(SWAP)
            .arg("swap")
            .arg("999999")
            .arg(&old)
            .arg(&new)
            .arg(&tmp)
            .arg("0")
            .status()
            .unwrap();
        assert!(ok.success());
        assert_eq!(std::fs::read_to_string(old.join("version.txt")).unwrap(), "new");
        assert!(!apps.join("App.app.old").exists(), "старый бандл убирается");
        assert!(!tmp.exists(), "временная папка убирается");
    }

    #[test]
    fn writable_probe_does_not_leave_files() {
        let dir = Dir::new("probe");
        assert!(writable(&dir.0));
        assert!(std::fs::read_dir(&dir.0).unwrap().count() == 0);
    }

    /// Последний релиз с GitHub → `Release` с адресом `.dmg`.
    fn latest_release() -> Option<Release> {
        let api = env!("CARGO_PKG_REPOSITORY").replace("https://github.com/", "https://api.github.com/repos/");
        let v: serde_json::Value = crate::update::http().ok()?.get(format!("{api}/releases/latest")).send().ok()?.json().ok()?;
        let tag = v.get("tag_name")?.as_str()?.trim_start_matches('v').to_string();
        let dmg = v.get("assets")?.as_array()?.iter().find_map(|a| {
            let name = a.get("name")?.as_str()?;
            if !(name.starts_with("yamusic-rs-") && name.ends_with("-macos.dmg")) {
                return None;
            }
            Some((
                a.get("browser_download_url")?.as_str()?.to_string(),
                a.get("size")?.as_u64()?,
                a.get("digest")?.as_str()?.strip_prefix("sha256:")?.to_string(),
            ))
        })?;
        Some(Release { version: tag, page: String::new(), dmg: Some(dmg) })
    }

    /// Настоящий релиз: скачать, сверить, смонтировать, скопировать, проверить подпись.
    /// Установленное приложение не трогает: `cargo test -- --ignored prepare_latest_release`.
    #[test]
    #[ignore = "ходит в сеть и монтирует образ"]
    fn prepare_latest_release() {
        let release = latest_release().expect("у последнего релиза есть .dmg с digest");
        let (staged, tmp) = prepare(&release).expect("образ скачался, смонтировался и проверился");
        assert!(staged.join("Contents/Info.plist").is_file(), "в образе есть Info.plist");
        assert!(!bundle_id(&staged).expect("identifier").is_empty());
        assert!(signed(&staged).is_ok(), "подпись нового бандла проверяется");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn copy_dir_copies_nested_files() {
        let dir = Dir::new("copy");
        let src = dir.0.join("src");
        let dst = dir.0.join("dst");
        std::fs::create_dir_all(src.join("Contents/MacOS")).unwrap();
        std::fs::write(src.join("Contents/MacOS/bin"), b"x").unwrap();
        copy_dir(&src, &dst).unwrap();
        assert_eq!(std::fs::read(dst.join("Contents/MacOS/bin")).unwrap(), b"x");
    }
}
