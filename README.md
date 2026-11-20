<div align="center">

<img src="docs/logo.png" width="96" height="96" alt="">

# Яндекс Музыка RS — сборка для macOS

**Неофициальный клиент Яндекс Музыки на Rust и Slint. Этот форк добавляет macOS:
родное окно, Keychain, медиаклавиши, автозапуск.**

![macOS 11+ arm64](https://img.shields.io/badge/macOS-11%2B%20arm64-2b2b30?labelColor=1c1c1f)
[![GPL-3.0](https://img.shields.io/badge/%D0%BB%D0%B8%D1%86%D0%B5%D0%BD%D0%B7%D0%B8%D1%8F-GPL--3.0-2b2b30?labelColor=1c1c1f)](LICENSE)
[![upstream](https://img.shields.io/badge/upstream-SpolzerMod%2Fyamusic--rs-2b2b30?labelColor=1c1c1f)](https://github.com/SpolzerMod/yamusic-rs)

[Что это](#что-это) · [Что добавлено](#что-добавлено-для-macos) · [Состояние](#состояние) ·
[Установка](#установка) · [Ограничения](#ограничения) · [Сборка](#сборка-и-проверки) ·
[English](#english)

</div>

> [!NOTE]
> Это форк [SpolzerMod/yamusic-rs](https://github.com/SpolzerMod/yamusic-rs). Основной репозиторий
> поддерживает Windows и Linux; macOS туда не принимают — у автора нет мака, проверить сборку ему
> нечем. Здесь macOS добавлен третьей платформой, а остальной код синхронизируется с upstream.

## Что это

Приложение рисует окно само, на процессоре: Rust, Slint и Skia, без Electron и Chromium.
Замеры автора upstream на Windows: один процесс и около 65 МБ памяти против 850 МБ и семи
процессов у официального клиента, 26 МБ на диске против 356 МБ. Полное описание возможностей,
замеры и FAQ — в снимке README основного репозитория:
[docs/upstream-readme-v1.1.2.md](docs/upstream-readme-v1.1.2.md).

На macOS тот же подход: программный рендерер Skia, без Metal и драйверов видеокарты.

Нужен аккаунт Яндекса, для полных треков — подписка Плюс. Проект не связан с Яндексом,
товарные знаки принадлежат их владельцам.

## Что добавлено для macOS

| | |
| --- | --- |
| Окно | Системная рамка, прозрачный заголовок, контент заходит под него, родной «светофор». Свои кнопки окна на macOS не рисуются, кнопка мини-плеера остаётся |
| Токен | «Связка ключей» (Keychain): в `config.json` только имя записи |
| Медиа | Медиаклавиши, AirPods и «Исполняется» в Пункте управления — `MPRemoteCommandCenter` и `MPNowPlayingInfoCenter`, с обложкой |
| Тема | «Как в системе» переключается на ходу: winit присылает `ThemeChanged`, опрос не нужен |
| Автозапуск | LaunchAgent `~/Library/LaunchAgents/ru.devcv.yamusic-rs.plist`; запускается бандл через `open -a`, а не файл внутри `Contents/MacOS` |
| Трей | Значок в строке меню (`NSStatusItem` на objc2, системный символ): показать/скрыть окно, играть/пауза, соседние треки, выход |
| Обновления | Приложение само скачивает `.dmg` из релиза, сверяет SHA-256, проверяет подпись и подменяет `.app` после выхода. История — в [CHANGELOG.md](CHANGELOG.md) |
| Диалоги | Выбор картинки и папки через `osascript`, буфер обмена через `arboard` |
| Мелочи | Ссылки открываются только по `https://`, память возвращается системе через `malloc_zone_pressure_relief`, мини-плеер встаёт по видимой области экрана — без строки меню и Dock |

## Состояние

Проверено на macOS 27.2.1, Apple Silicon:

- сборка и запуск `.app`: окно с «светофором», страницы, шрифты с music.yandex.ru, экран входа с кодом устройства;
- `cargo test` (28 тестов, 4 помечены `ignored`), `clippy --all-targets -- -D warnings`;
- `plutil -lint` и `codesign --verify --deep --strict` для бандла;
- запись, чтение и удаление токена в Keychain (тест `--ignored keychain`).

Вживую ещё не проверено — нужен вход в аккаунт и играющий трек: сохранение токена из интерфейса,
медиаклавиши и Пункт управления, скрытие «светофора» в мини-плеере, автозапуск после перезагрузки.
Intel не собирался и не проверялся.

## Установка

Готовый `.dmg` — в [релизах](https://github.com/M1kage-dev/yamusic-rs/releases/latest)
(универсальный: Apple Silicon и Intel, SHA-256 в описании релиза). Собрать самому — один шаг:

```bash
git clone https://github.com/M1kage-dev/yamusic-rs
cd yamusic-rs
cargo build --release
tools/macos-app.sh
open "target/macos/Яндекс Музыка RS.app"
```

`tools/macos-app.sh` собирает бандл: `Info.plist`, значок из `icons/app.svg`, ad-hoc подпись.
Бандл можно перенести в `/Applications` — автозапуск найдёт его по новому пути.

Первый запуск: macOS спросит про сеть (Tiny Shield или системный фаервол), затем приложение
скачает шрифты и иконки с music.yandex.ru. Вход по коду: приложение показывает код, вы вводите его
на [ya.ru/device](https://ya.ru/device) в любом браузере.

Если бандл скачан из интернета, а не собран локально, macOS повесит на него карантин:

```bash
xattr -dr com.apple.quarantine "/Applications/Яндекс Музыка RS.app"
```

То же самое делает «Системные настройки → Конфиденциальность и безопасность → Всё равно открыть».

### Где что лежит

| Что | Путь |
| --- | --- |
| Настройки | `~/Library/Application Support/yamusic-rs/config.json` |
| Шрифты и иконки из веб-версии | `~/Library/Caches/yamusic-rs/assets` |
| Скачанные треки | `~/Music/Яндекс Музыка` (меняется в настройках) |
| Токен | «Связка ключей», service `yamusic-rs`, записи `token` и `refresh` |
| Автозапуск | `~/Library/LaunchAgents/ru.devcv.yamusic-rs.plist` |
| Журнал | `~/Library/Application Support/yamusic-rs/log.txt` |

## Ограничения

- **Закрытие окна закрывает приложение**: значок в строке меню даёт быстрый доступ к окну
  и плееру, но окно в трей не прячется.
- **Только русский интерфейс**: словари en/uz/kk лежат в официальном клиенте, которого для macOS
  нет. В веб-версии их тоже нет — проверено: 80 начальных чанков, только использования ключей,
  без самих словарей.
- **Подпись ad-hoc**: после каждого обновления macOS один раз спросит пароль за токеном в связке ключей.
- **Intel** не проверялся.

## Сборка и проверки

Нужны Xcode Command Line Tools (`xcode-select --install`) и Rust 1.92+.

```bash
cargo build --release            # бинарь под текущую архитектуру
cargo test                       # 33 теста; 4 ignored требуют сети, app.asar или Keychain
cargo clippy --all-targets -- -D warnings
tools/macos-app.sh --skip-build  # бандл из готового бинаря
tools/macos-app.sh --universal --dmg  # arm64 + x86_64 одним бинарём и .dmg
```

GitHub Actions на каждый push собирает то же самое: `windows`, `linux` и `macos` (универсальный
бинарь, проверка подписи, `lipo -archs`), а на тег `v*` прикладывает `.dmg` к релизу.

`cargo fmt` не включён: стиль upstream шире стандартного, массовый реформат сломал бы слияния
с основным репозиторием.

## Связь с основным репозиторием

```bash
git remote add upstream https://github.com/SpolzerMod/yamusic-rs
git fetch upstream && git merge upstream/main
```

Мерж, а не ребейз: история остаётся общей с upstream. Платформенный слой macOS лежит в отдельных
файлах (`src/sys/macos.rs`, `src/sys/unix.rs`, `src/nowplaying.rs`, `tools/macos-app.sh`), общие
файлы правятся маленькими `cfg`-блоками — поэтому конфликтов мало. `README.md` в форке свой:
при конфликте остаётся наша версия.

## Версии

Номер — своя линия, база апстрима — в метаданных: `X.Y.Z+upstream.A.B.C`.

| Версия | Что внутри |
| --- | --- |
| `1.1.2` | первый релиз порта (до введения схемы) |
| `1.0.1+upstream.1.1.2` | первая сборка по схеме: наша линия 1.0.1, внутри апстрим 1.1.2 |
| `1.0.2+upstream.1.1.2` | наше исправление на той же базе |
| `1.1.0+upstream.1.2.0` | вобрали апстрим 1.2.0 и добавили своё |

Что изменилось в каждой версии — [CHANGELOG.md](CHANGELOG.md); его раздел для тега становится
описанием релиза, без раздела релиз не собирается.

Порядок сравнивается только по первой тройке (`src/update.rs`): хвост `+upstream.A.B.C` —
метаданные, они говорят, какой апстрим внутри, и на обновления не влияют. Поэтому каждый
релиз поднимает сам номер. В Finder и «О программе» macOS показывает `1.0.1`: CFBundle
требует только цифры и точки, полная строка живёт в `Cargo.toml` и в описании релиза.

Первая сборка по схеме численно меньше уже выпущенной `1.1.2`, поэтому её ставят вручную
один раз: тоста «Доступна версия» не будет. Со следующей (`1.0.2`) всё работает как обычно.

## Лицензия и благодарности

GPL-3.0, как и upstream: изменённая версия распространяется под той же лицензией и с исходным
кодом. Автор исходного проекта — [Spolzer](https://github.com/SpolzerMod). Шестерёнка в логотипе —
логотип Rust от Rust Foundation (CC-BY 4.0). Схема медиаклавиш повторяет порт из
[PR #11](https://github.com/SpolzerMod/yamusic-rs/pull/11) (jkolobok).

## English

Fork of [SpolzerMod/yamusic-rs](https://github.com/SpolzerMod/yamusic-rs) that adds macOS.
The same Rust + Slint + Skia client: native window frame with traffic lights, Keychain for the
token, media keys and Now Playing, live system theme, LaunchAgent autostart.

```bash
git clone https://github.com/M1kage-dev/yamusic-rs
cd yamusic-rs
cargo build --release && tools/macos-app.sh
open "target/macos/Яндекс Музыка RS.app"
```

Verified on macOS 27.2.1 (Apple Silicon): the `.app` builds and launches, pages load, web fonts
download, device-code login screen, `cargo test`, `clippy -D warnings`, Keychain roundtrip.
Not verified yet: login from the UI, media keys on a playing track, autostart after reboot.
No tray, no auto-update, Russian UI only, ad-hoc signature (macOS asks for the Keychain password
once after each update), Intel untested. Requires Xcode Command Line Tools and Rust 1.92+.
GPL-3.0.
