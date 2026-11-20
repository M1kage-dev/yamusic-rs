//! Где хранить токен. Windows: шифрование DPAPI, расшифровать может только та же учётная
//! запись на том же компьютере, в config.json лежит base64 шифротекста. Linux: связка ключей
//! пользователя через Secret Service, macOS: связка ключей Keychain. В обоих случаях
//! в config.json только имя записи.

#[cfg(windows)]
use base64::Engine as _;

/// Дополнительная «соль» DPAPI: другие программы того же пользователя без неё не расшифруют.
/// Старое имя оставлено намеренно: с другой солью сохранённые токены перестанут читаться.
#[cfg(windows)]
const ENTROPY: &[u8] = b"yamusic-lite token v1";

#[cfg(windows)]
mod dpapi {
    use super::ENTROPY;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    fn blob(b: &[u8]) -> CRYPT_INTEGER_BLOB {
        CRYPT_INTEGER_BLOB { cbData: b.len() as u32, pbData: b.as_ptr() as *mut u8 }
    }

    fn take(out: CRYPT_INTEGER_BLOB) -> Vec<u8> {
        let v = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize) }.to_vec();
        unsafe { LocalFree(out.pbData as _) };
        v
    }

    pub fn protect(data: &[u8]) -> Option<Vec<u8>> {
        let (input, entropy) = (blob(data), blob(ENTROPY));
        let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
        let ok = unsafe {
            CryptProtectData(&input, std::ptr::null(), &entropy, std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut out)
        };
        (ok != 0).then(|| take(out))
    }

    pub fn unprotect(data: &[u8]) -> Option<Vec<u8>> {
        let (input, entropy) = (blob(data), blob(ENTROPY));
        let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
        let ok = unsafe {
            CryptUnprotectData(&input, std::ptr::null_mut(), &entropy, std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut out)
        };
        (ok != 0).then(|| take(out))
    }
}

/// Связка ключей Linux через Secret Service (GNOME Keyring, KWallet, KeePassXC). Секрет лежит
/// там, в config.json остаётся только имя записи. Сессия шифрованная (Диффи — Хеллман и AES),
/// так что по шине D-Bus секрет открытым не идёт.
#[cfg(target_os = "linux")]
mod keyring {
    use crate::config::log;
    use secret_service::blocking::SecretService;
    use secret_service::EncryptionType;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;

    /// Что уже лежит в связке: те же значения повторно не пишем. Конфиг сохраняется на каждое
    /// изменение настроек, а запись в связку — это запрос по D-Bus.
    static STORED: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);
    /// Службы нет (нет шины или демона): до конца работы больше не пытаемся.
    static MISSING: AtomicBool = AtomicBool::new(false);

    fn attributes(name: &str) -> HashMap<&str, &str> {
        HashMap::from([("application", "yamusic-rs"), ("secret", name)])
    }

    fn label(name: &str) -> String {
        let what = if name == "refresh" { "токен обновления" } else { "токен входа" };
        format!("{}: {what}", env!("APP_NAME"))
    }

    fn with<R>(f: impl FnOnce(&SecretService) -> Result<R, secret_service::Error>) -> Option<R> {
        if MISSING.load(Ordering::Relaxed) {
            return None;
        }
        let ss = match SecretService::connect(EncryptionType::Dh) {
            Ok(ss) => ss,
            Err(e) => {
                MISSING.store(true, Ordering::Relaxed);
                log(format!("связка ключей недоступна, токен останется в config.json: {e}"));
                return None;
            }
        };
        f(&ss).map_err(|e| log(format!("связка ключей: {e}"))).ok()
    }

    fn cached(f: impl FnOnce(&mut HashMap<String, String>)) {
        f(STORED.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new));
    }

    pub fn store(name: &str, secret: &str) -> bool {
        let mut same = false;
        cached(|m| same = m.get(name).is_some_and(|v| v == secret));
        if same {
            return true;
        }
        let saved = with(|ss| {
            let c = ss.get_default_collection().or_else(|_| ss.get_any_collection())?;
            c.ensure_unlocked()?;
            c.create_item(&label(name), attributes(name), secret.as_bytes(), true, "text/plain")?;
            Ok(())
        });
        if saved.is_some() {
            cached(|m| {
                m.insert(name.into(), secret.into());
            });
        }
        saved.is_some()
    }

    pub fn load(name: &str) -> Option<String> {
        let secret = with(|ss| {
            let found = ss.search_items(attributes(name))?;
            let Some(item) = found.unlocked.first().or(found.locked.first()) else { return Ok(None) };
            item.ensure_unlocked()?;
            Ok(String::from_utf8(item.get_secret()?).ok())
        })??;
        cached(|m| {
            m.insert(name.into(), secret.clone());
        });
        Some(secret)
    }

    pub fn remove(name: &str) {
        let mut known = false;
        cached(|m| known = m.remove(name).is_some());
        if known {
            with(|ss| ss.search_items(attributes(name))?.unlocked.iter().try_for_each(|i| i.delete()));
        }
    }
}

/// Токены на macOS лежат файлом в папке настроек, доступном только владельцу (0600).
///
/// Почему не связка ключей: ACL записи привязан к конкретной сборке, и любой новый бинарь
/// просит доступ заново — ни ad-hoc подпись, ни собственный сертификат этого не меняют
/// («Всегда разрешать» после обновления не действует). Пользователь получал по два диалога
/// на каждое обновление. Файл с правами 0600 читает только его учётная запись: для клиента,
/// которому нужен один токен Яндекса, этого достаточно, а диалогов нет вообще.
#[cfg(target_os = "macos")]
mod keychain {
    use crate::config::log;
    use std::collections::BTreeMap;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Mutex;

    /// Разобранное содержимое файла: читаем один раз за запуск, дальше работаем с памятью.
    static CACHE: Mutex<Option<BTreeMap<String, String>>> = Mutex::new(None);

    fn path() -> std::path::PathBuf {
        crate::config::dir().join("secrets.json")
    }

    fn read_file() -> BTreeMap<String, String> {
        let Ok(text) = std::fs::read_to_string(path()) else { return BTreeMap::new() };
        match serde_json::from_str(&text) {
            Ok(map) => map,
            Err(e) => {
                log(format!("файл токенов повреждён: {e}"));
                BTreeMap::new()
            }
        }
    }

    /// Пишем через временный файл: права 0600 выставляются до переименования, чтобы токен
    /// ни мгновения не лежал читаемым для других.
    fn write_file(map: &BTreeMap<String, String>) -> bool {
        let json = match serde_json::to_vec_pretty(map) {
            Ok(j) => j,
            Err(e) => {
                log(format!("токены: {e}"));
                return false;
            }
        };
        let file = path();
        // Папки настроек может ещё не быть (первый запуск, чистый CI): создаём до записи.
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = file.with_extension("json.tmp");
        let saved = std::fs::write(&tmp, &json).is_ok()
            && std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600)).is_ok()
            && std::fs::rename(&tmp, &file).is_ok();
        if !saved {
            log(format!("токены: не удалось записать {}", file.display()));
            let _ = std::fs::remove_file(&tmp);
        }
        saved
    }

    fn with_map<R>(f: impl FnOnce(&mut BTreeMap<String, String>) -> R) -> R {
        let mut guard = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        if guard.is_none() {
            *guard = Some(read_file());
        }
        f(guard.as_mut().expect("карта заполнена выше"))
    }

    pub fn store(name: &str, secret: &str) -> bool {
        with_map(|m| {
            if m.get(name).is_some_and(|v| v == secret) {
                return true;
            }
            m.insert(name.to_string(), secret.to_string());
            write_file(m)
        })
    }

    pub fn load(name: &str) -> Option<String> {
        with_map(|m| m.get(name).cloned())
    }

    pub fn remove(name: &str) {
        with_map(|m| {
            m.remove(name);
            if m.is_empty() {
                let _ = std::fs::remove_file(path());
            } else {
                write_file(m);
            }
        });
    }
}

/// Префикс в config.json: секрет лежит в связке ключей, а не рядом.
#[cfg(any(target_os = "linux", target_os = "macos"))]
const IN_KEYRING: &str = "keyring:";

/// Спрятать секрет для хранения в конфиге. `name` различает секреты в связке ключей Linux,
/// DPAPI на Windows он не нужен. None — спрятать негде.
pub fn seal(name: &str, plain: &str) -> Option<String> {
    #[cfg(windows)]
    {
        let _ = name;
        dpapi::protect(plain.as_bytes()).map(|b| base64::engine::general_purpose::STANDARD.encode(b))
    }
    #[cfg(target_os = "linux")]
    {
        keyring::store(name, plain).then(|| format!("{IN_KEYRING}{name}"))
    }
    #[cfg(target_os = "macos")]
    {
        keychain::store(name, plain).then(|| format!("{IN_KEYRING}{name}"))
    }
}

/// Достать секрет из конфига; None — чужая учётная запись, нет записи или испорченные данные.
pub fn open(sealed: &str) -> Option<String> {
    #[cfg(windows)]
    {
        let raw = base64::engine::general_purpose::STANDARD.decode(sealed.trim()).ok()?;
        String::from_utf8(dpapi::unprotect(&raw)?).ok()
    }
    #[cfg(target_os = "linux")]
    {
        keyring::load(sealed.trim().strip_prefix(IN_KEYRING)?)
    }
    #[cfg(target_os = "macos")]
    {
        keychain::load(sealed.trim().strip_prefix(IN_KEYRING)?)
    }
}

/// Секрет больше не нужен (вышли из аккаунта). DPAPI ничего не хранит, связку ключей чистим.
pub fn forget(name: &str) {
    #[cfg(target_os = "linux")]
    keyring::remove(name);
    #[cfg(target_os = "macos")]
    keychain::remove(name);
    #[cfg(windows)]
    let _ = name;
}

#[cfg(all(test, windows))]
mod tests {
    #[test]
    fn roundtrip() {
        let s = super::seal("token", "y0_secret").unwrap();
        assert!(!s.contains("y0_secret"));
        assert_eq!(super::open(&s).as_deref(), Some("y0_secret"));
        assert_eq!(super::open("AAAA"), None);
    }
}

/// Файл секретов: запись, чтение, удаление и права 0600.
#[cfg(all(test, target_os = "macos"))]
mod keychain_tests {
    #[test]
    fn roundtrip_through_file() {
        let sealed = super::seal("test-token", "y0_secret").expect("секрет должен лечь в файл");
        assert!(sealed.starts_with("keyring:"), "в config.json попадает только имя записи: {sealed}");
        assert!(!sealed.contains("y0_secret"));
        assert_eq!(super::open(&sealed).as_deref(), Some("y0_secret"));
        super::forget("test-token");
        assert_eq!(super::open(&sealed), None, "после forget секрета нет");
    }

    /// Главная защита файлового хранения — права: читает только владелец.
    #[test]
    fn secrets_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let sealed = super::seal("perm-token", "x").expect("секрет записался");
        let path = crate::config::dir().join("secrets.json");
        let mode = std::fs::metadata(&path).expect("файл есть").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "файл токенов доступен только владельцу");
        super::forget("perm-token");
        assert_eq!(super::open(&sealed), None);
    }
}
