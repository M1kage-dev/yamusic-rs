//! macOS: окно через winit, папки ~/Library, один экземпляр через unix-сокет, диалоги
//! через osascript, автозапуск через LaunchAgent. Трея нет: закрытие окна закрывает
//! приложение, как и на Linux.

use slint::winit_030::WinitWindowAccessor;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[path = "unix.rs"]
mod unix;
pub use unix::{claim_instance, cleanup, on_wake, wake_existing};

/// Bundle identifier из Info.plist (tools/macos-app.sh) и метка LaunchAgent.
const BUNDLE_ID: &str = "ru.devcv.yamusic-rs";

pub fn created(w: &slint::Window) -> bool {
    w.with_winit_window(|_| ()).is_some()
}

/// Значок в Dock берётся из бандла .app: у окна на macOS своего значка нет.
pub fn prepare(_: &slint::Window) {}

/// Показать или спрятать «светофор» (закрыть, свернуть, развернуть). В мини-плеере
/// свои кнопки, а системные закрыли бы обложку.
pub fn set_window_buttons(w: &slint::Window, visible: bool) {
    use objc2_app_kit::{NSView, NSWindowButton};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    w.with_winit_window(|w| {
        let Ok(handle) = w.window_handle() else { return };
        let RawWindowHandle::AppKit(h) = handle.as_raw() else { return };
        // SAFETY: winit отдаёт живой NSView своего окна, вызов идёт из главного потока.
        let view: &NSView = unsafe { h.ns_view.cast().as_ref() };
        let Some(window) = view.window() else { return };
        for kind in [NSWindowButton::CloseButton, NSWindowButton::MiniaturizeButton, NSWindowButton::ZoomButton] {
            if let Some(b) = window.standardWindowButton(kind) {
                b.setHidden(!visible);
            }
        }
    });
}

/// winit на macOS вместе с фокусом активирует и само приложение.
pub fn raise(w: &slint::Window) {
    w.set_minimized(false);
    w.with_winit_window(|w| w.focus_window());
}

pub fn scale(w: &slint::Window) -> f32 {
    w.with_winit_window(|w| w.scale_factor() as f32).unwrap_or(1.0)
}

/// Видимая область экрана без строки меню и Dock: по ней встаёт мини-плеер, иначе
/// плашка уезжает под Dock. NSScreen считает в точках от левого нижнего угла главного
/// экрана, winit — в пикселях от левого верхнего; переводим через высоту главного экрана.
pub fn work_area(w: &slint::Window) -> (i32, i32, i32, i32) {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSScreen, NSView};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let visible = w
        .with_winit_window(|ww| {
            let mtm = MainThreadMarker::new()?;
            let handle = ww.window_handle().ok()?;
            let RawWindowHandle::AppKit(h) = handle.as_raw() else { return None };
            // SAFETY: главный поток, окно живо.
            let view: &NSView = unsafe { h.ns_view.cast().as_ref() };
            let screen = view.window()?.screen()?;
            let f = screen.visibleFrame();
            let primary = NSScreen::screens(mtm).firstObject()?.frame();
            let scale = ww.scale_factor();
            let top = primary.size.height - (f.origin.y + f.size.height);
            Some((
                (f.origin.x * scale).round() as i32,
                (top * scale).round() as i32,
                ((f.origin.x + f.size.width) * scale).round() as i32,
                ((top + f.size.height) * scale).round() as i32,
            ))
        })
        .flatten();
    visible.unwrap_or_else(|| {
        // Окно ещё не создано или экрана нет — берём монитор целиком.
        w.with_winit_window(|w| {
            let m = w.current_monitor()?;
            let (p, s) = (m.position(), m.size());
            Some((p.x, p.y, p.x + s.width as i32, p.y + s.height as i32))
        })
        .flatten()
        .unwrap_or_else(|| {
            let s = w.size();
            (0, 0, s.width as i32, s.height as i32)
        })
    })
}

/// AppleScript-диалог. Заголовок и папка уходят аргументами, а не в текст скрипта:
/// кавычки и юникод в них ничего не сломают. Отмена — osascript завершается с ошибкой.
fn osascript(script: &str, args: &[&str]) -> Option<String> {
    let out = Command::new("osascript").arg("-e").arg(script).args(args).output().ok()?;
    if !out.status.success() {
        // -128 — пользователь нажал «Отмена», это не ошибка. Всё остальное (нет прав, битый
        // скрипт) раньше терялось вместе с подавленным stderr и выглядело как отмена.
        let err = String::from_utf8_lossy(&out.stderr);
        let err = err.trim();
        if !err.is_empty() && !err.contains("-128") {
            crate::config::log(format!("диалог не открылся: {err}"));
        }
        return None;
    }
    let path = String::from_utf8_lossy(&out.stdout).trim_end_matches('\n').to_string();
    (!path.is_empty()).then_some(path)
}

pub fn pick_image(_: &slint::Window, title: &str) -> Option<String> {
    osascript(
        r#"on run argv
  POSIX path of (choose file with prompt (item 1 of argv) of type {"public.jpeg", "public.png"})
end run"#,
        &[title],
    )
}

pub fn pick_folder(_: &slint::Window, title: &str, start: &str) -> Option<String> {
    let start = if Path::new(start).is_dir() { start.to_string() } else { home().display().to_string() };
    osascript(
        r#"on run argv
  POSIX path of (choose folder with prompt (item 1 of argv) default location (POSIX file (item 2 of argv)))
end run"#,
        &[title, &start],
    )
    // Корень диска — это "/", срезать у него косую нельзя: выбор станет пустым.
    .map(|p| if p == "/" { p } else { p.trim_end_matches('/').to_string() })
    .filter(|p| !p.is_empty())
}

/// Открыть ссылку в браузере. Только https: адрес приходит с сервера и из настроек,
/// а `open` умеет запускать и локальные файлы — этому пути доверять нельзя.
pub fn open_url(url: &str) {
    if !url.starts_with("https://") {
        crate::config::log(format!("ссылка не открыта, разрешён только https: {url}"));
        return;
    }
    if let Ok(mut child) = Command::new("open").arg(url).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
        std::thread::spawn(move || child.wait());
    }
}

/// Вернуть системе свободные страницы malloc, как malloc_trim на Linux.
pub fn trim_memory() {
    extern "C" {
        fn malloc_zone_pressure_relief(zone: *mut std::ffi::c_void, goal: usize) -> usize;
    }
    // SAFETY: null — «все зоны»; функции всё равно, какие страницы освобождать.
    unsafe {
        malloc_zone_pressure_relief(std::ptr::null_mut(), 0);
    }
}

pub fn release_memory() {
    trim_memory();
}

/// `AppleInterfaceStyle = Dark` есть только в тёмном оформлении.
fn is_dark(style: &str) -> bool {
    style.trim().eq_ignore_ascii_case("dark")
}

pub fn system_light_theme() -> bool {
    let out = Command::new("defaults").args(["read", "-g", "AppleInterfaceStyle"]).stderr(Stdio::null()).output();
    !matches!(out, Ok(o) if o.status.success() && is_dark(&String::from_utf8_lossy(&o.stdout)))
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()))
}

pub fn config_base() -> PathBuf {
    home().join("Library").join("Application Support")
}

pub fn cache_base() -> PathBuf {
    home().join("Library").join("Caches")
}

/// Finder показывает её как «Музыка», но на диске она всегда Music.
pub fn music_dir() -> PathBuf {
    home().join("Music")
}

/// Имя компьютера из «Основные → Общий доступ», а не сетевое имя хоста:
/// его Яндекс показывает в списке устройств аккаунта.
pub fn hostname() -> Option<String> {
    let out = Command::new("scutil").args(["--get", "ComputerName"]).stderr(Stdio::null()).output().ok()?;
    let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !name.is_empty()).then_some(name)
}

pub fn copy_text(text: &str) {
    if let Ok(mut c) = arboard::Clipboard::new() {
        let _ = c.set_text(text);
    }
}

fn agent_file() -> PathBuf {
    home().join("Library").join("LaunchAgents").join(format!("{BUNDLE_ID}.plist"))
}

pub fn autostart_enabled() -> bool {
    agent_file().exists()
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// `.app`, из которого запущено приложение. `None` — запущено не из бандла (`cargo run`,
/// файл внутри `Contents/MacOS`): автообновление такому процессу не предлагаем.
pub fn bundle_path() -> Option<PathBuf> {
    bundle_of(&std::env::current_exe().ok()?)
}

/// `.app`, внутри которого лежит бинарь: у бандла запускать надо его, а не файл в
/// Contents/MacOS — иначе теряются значок и активация.
fn bundle_of(exe: &Path) -> Option<PathBuf> {
    let s = exe.to_string_lossy();
    let i = s.find(".app/Contents/MacOS/")?;
    Some(PathBuf::from(&s[..i + 4]))
}

/// LaunchAgent: launchd запускает приложение при входе в систему. Без трея
/// запускаемся с окном, а не скрытыми.
fn agent_plist(exe: &Path) -> String {
    let escape = |s: &str| xml_escape(s);
    let program = match bundle_of(exe) {
        Some(app) => format!(
            "    <string>/usr/bin/open</string>\n    <string>-a</string>\n    <string>{}</string>",
            escape(&app.to_string_lossy())
        ),
        None => format!("    <string>{}</string>", escape(&exe.to_string_lossy())),
    };
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{BUNDLE_ID}</string>
  <key>ProgramArguments</key>
  <array>
{program}
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>ProcessType</key>
  <string>Interactive</string>
</dict>
</plist>
"#
    )
}

pub fn set_autostart(on: bool) -> bool {
    let file = agent_file();
    if !on {
        return std::fs::remove_file(&file).is_ok() || !file.exists();
    }
    let Ok(exe) = std::env::current_exe() else { return false };
    if bundle_of(&exe).is_none() {
        // Запуск из target/ или из перенесённого бандла: путь в агенте может исчезнуть.
        crate::config::log(format!("автозапуск указывает на файл вне .app: {}", exe.display()));
    }
    let plist = agent_plist(&exe);
    // Конфиг сохраняется на каждое изменение настроек: не переписываем тот же файл.
    if std::fs::read_to_string(&file).is_ok_and(|old| old == plist) {
        return true;
    }
    file.parent().is_some_and(|d| std::fs::create_dir_all(d).is_ok()) && std::fs::write(&file, plist).is_ok()
}

/// Приложение могли перенести в другую папку: путь в LaunchAgent обновляем.
pub fn refresh_autostart() {
    if autostart_enabled() {
        set_autostart(true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xml_special_chars_are_escaped() {
        assert_eq!(xml_escape("a & b < c > \"d\""), "a &amp; b &lt; c &gt; &quot;d&quot;");
        assert_eq!(xml_escape("Яндекс Музыка RS"), "Яндекс Музыка RS");
    }

    #[test]
    fn bundle_path_is_found_from_the_executable() {
        let inside = Path::new("/Applications/Яндекс Музыка RS.app/Contents/MacOS/yamusic-rs");
        assert_eq!(bundle_of(inside), Some(PathBuf::from("/Applications/Яндекс Музыка RS.app")));
        assert_eq!(bundle_of(Path::new("/Users/x/target/release/yamusic-rs")), None);
    }

    /// Запускать бандл надо через `open -a`: прямой запуск файла из Contents/MacOS
    /// теряет значок и активацию.
    #[test]
    fn agent_starts_the_bundle_through_open() {
        let exe = Path::new("/Applications/Яндекс Музыка RS.app/Contents/MacOS/yamusic-rs");
        let plist = agent_plist(exe);
        assert!(plist.contains("<string>/usr/bin/open</string>"), "бандл запускается через open: {plist}");
        assert!(plist.contains("<string>-a</string>"));
        assert!(plist.contains("<string>/Applications/Яндекс Музыка RS.app</string>"));
        assert!(!plist.contains("Contents/MacOS"), "внутренний файл бандла в агент не попадает");
        assert!(plist.contains("<key>RunAtLoad</key>"));
        assert!(plist.contains(BUNDLE_ID));
    }

    /// Бинарь вне бандла (cargo run) — агент запускает его напрямую.
    #[test]
    fn agent_runs_a_plain_binary_directly() {
        let plist = agent_plist(Path::new("/Users/x/target/release/yamusic-rs"));
        assert!(plist.contains("<string>/Users/x/target/release/yamusic-rs</string>"));
        assert!(!plist.contains("/usr/bin/open"));
    }

    #[test]
    fn autostart_agent_is_valid_xml() {
        let plist = agent_plist(Path::new("/tmp/a & b.app/Contents/MacOS/yamusic-rs"));
        assert!(plist.contains("<string>/tmp/a &amp; b.app</string>"), "амперсанд в пути экранируется: {plist}");
    }

    #[test]
    fn dark_style_detection() {
        assert!(is_dark("Dark\n"));
        assert!(is_dark("dark"));
        assert!(!is_dark(""));
        assert!(!is_dark("Light"));
    }
}
