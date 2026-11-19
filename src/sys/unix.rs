//! Один экземпляр приложения на Linux и macOS: unix-сокет в `XDG_RUNTIME_DIR`
//! (на macOS переменной нет — во временной папке). Второй запуск соединяется с сокетом
//! и просит работающее окно показаться.

use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Как app_id в main.rs и имя .desktop в пакете Linux.
pub const APP_ID: &str = "yamusic-rs";

fn socket_path() -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(dir) => PathBuf::from(dir).join(format!("{APP_ID}.sock")),
        None => std::env::temp_dir().join(format!("{APP_ID}-{}.sock", std::env::var("USER").unwrap_or_default())),
    }
}

/// Что вышло из попытки занять сокет.
enum Claim {
    /// Сокет наш: слушатель нужно сохранить, иначе файл останется без владельца.
    Mine(UnixListener),
    /// Другой экземпляр жив и отвечает на соединения.
    Taken,
    /// Нет доступа к папке с сокетом: работаем без проверки второго запуска.
    NoSocket,
}

/// Занять сокет по пути `path`. Сокет остаётся на диске и после падения приложения,
/// поэтому занятый, но молчащий файл считается брошенным и перезабирается.
fn claim_at(path: &Path) -> Claim {
    match UnixListener::bind(path) {
        Ok(l) => Claim::Mine(l),
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            if UnixStream::connect(path).is_ok() {
                return Claim::Taken;
            }
            let _ = std::fs::remove_file(path);
            match UnixListener::bind(path) {
                Ok(l) => Claim::Mine(l),
                Err(_) => Claim::NoSocket,
            }
        }
        Err(_) => Claim::NoSocket,
    }
}

/// Попросить экземпляр, слушающий `path`, показать окно. `false` — никто не ответил.
fn wake_at(path: &Path) -> bool {
    match UnixStream::connect(path) {
        Ok(mut s) => s.write_all(b"show").is_ok(),
        Err(_) => false,
    }
}

static LISTENER: Mutex<Option<UnixListener>> = Mutex::new(None);

/// `true` — этот процесс главный; `false` — уже работает другой, его надо разбудить.
pub fn claim_instance() -> bool {
    match claim_at(&socket_path()) {
        Claim::Mine(l) => {
            *LISTENER.lock().unwrap() = Some(l);
            true
        }
        Claim::Taken => false,
        Claim::NoSocket => true,
    }
}

/// Второй запуск: попросить работающий экземпляр показать окно.
pub fn wake_existing() {
    wake_at(&socket_path());
}

/// Звать `f` из фонового потока, когда второй запуск просит показать окно.
/// Пустое соединение — это проверка из `claim_at`, на неё не реагируем.
pub fn on_wake(f: impl Fn() + Send + 'static) {
    let Some(listener) = LISTENER.lock().unwrap().take() else { return };
    std::thread::spawn(move || {
        for mut s in listener.incoming().flatten() {
            let mut buf = [0u8; 4];
            if s.read(&mut buf).is_ok_and(|n| n > 0) {
                f();
            }
        }
    });
}

pub fn cleanup() {
    let _ = std::fs::remove_file(socket_path());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    /// Отдельная папка на тест: сокет не должен пересекаться с запущенным приложением.
    struct Dir(PathBuf);

    impl Dir {
        fn new(tag: &str) -> Dir {
            let p = std::env::temp_dir().join(format!("ym-unix-{tag}-{}", std::process::id()));
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

    fn mine(c: Claim) -> UnixListener {
        match c {
            Claim::Mine(l) => l,
            Claim::Taken => panic!("сокет должен достаться нам"),
            Claim::NoSocket => panic!("сокет должен быть доступен"),
        }
    }

    #[test]
    fn second_instance_is_refused() {
        let dir = Dir::new("second");
        let path = dir.0.join("app.sock");
        let _first = mine(claim_at(&path));
        assert!(matches!(claim_at(&path), Claim::Taken), "второй экземпляр должен получить отказ");
    }

    #[test]
    fn abandoned_socket_file_is_reclaimed() {
        let dir = Dir::new("stale");
        let path = dir.0.join("app.sock");
        // Так выглядит сокет после падения: файл есть, слушателя нет.
        std::fs::write(&path, b"").unwrap();
        let _ = mine(claim_at(&path));
    }

    #[test]
    fn wake_reaches_the_running_instance() {
        let dir = Dir::new("wake");
        let path = dir.0.join("app.sock");
        let listener = mine(claim_at(&path));
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for mut s in listener.incoming().flatten() {
                let mut buf = [0u8; 4];
                if s.read(&mut buf).is_ok_and(|n| n > 0) {
                    let _ = tx.send(());
                }
            }
        });
        assert!(wake_at(&path), "сигнал должен уйти");
        rx.recv_timeout(std::time::Duration::from_secs(5)).expect("работающий экземпляр должен получить сигнал");
    }

    #[test]
    fn wake_without_instance_reports_failure() {
        let dir = Dir::new("nobody");
        assert!(!wake_at(&dir.0.join("nobody.sock")));
    }
}
