//! Значок в строке меню macOS: быстрый доступ к окну и управлению плеером.
//! Свой NSStatusItem на objc2: сторонний крейт тянул бы вторую копию objc2 и собственный
//! цикл событий, а нам нужны только меню и один обработчик на все пункты.

use objc2::rc::Retained;
use objc2::runtime::NSObject;
use objc2::{define_class, msg_send, sel, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSCellImagePosition, NSControlStateValueOff, NSControlStateValueOn, NSImage, NSMenu, NSMenuItem, NSStatusBar,
    NSStatusItem, NSVariableStatusItemLength,
};
use objc2_foundation::{NSInteger, NSString};
use std::cell::RefCell;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Event {
    Toggle,
    PlayPause,
    Prev,
    Next,
    Quit,
}

/// Теги пунктов меню: по ним обработчик понимает, что нажали.
const TAG_TOGGLE: NSInteger = 1;
const TAG_PLAY: NSInteger = 2;
const TAG_PREV: NSInteger = 3;
const TAG_NEXT: NSInteger = 4;
const TAG_QUIT: NSInteger = 5;

/// Что делать по команде из меню: обработчик ставит gui, зовётся он в главном потоке.
type Handler = Box<dyn Fn(Event)>;

thread_local! {
    /// Обработчик команд. Меню зовёт его в главном потоке, поэтому Send не нужен.
    static HANDLER: RefCell<Option<Handler>> = const { RefCell::new(None) };
    /// Что держим живым: без ссылок на статус-итем и цель меню AppKit их выбросит.
    static ITEMS: RefCell<Option<Items>> = const { RefCell::new(None) };
}

/// Объект-цель меню: AppKit зовёт `menuClicked:` у него, тег говорит, какой пункт нажали.
struct TargetIvars;

define_class!(
    // SAFETY: NSObject не требует ничего от наследника, своих Drop и ivar-ов у Target нет.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "YmTrayTarget"]
    #[ivars = TargetIvars]
    struct Target;

    impl Target {
        #[unsafe(method(menuClicked:))]
        fn menu_clicked(&self, sender: &NSMenuItem) {
            if let Some(e) = event_of(sender.tag()) {
                dispatch(e);
            }
        }
    }
);

impl Target {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(TargetIvars);
        unsafe { msg_send![super(this), init] }
    }
}

struct Items {
    _status: Retained<NSStatusItem>,
    _target: Retained<Target>,
    toggle: Retained<NSMenuItem>,
    play: Retained<NSMenuItem>,
    /// Что уже показано в меню: заголовки меняем только при смене состояния, а не 4 раза в секунду.
    playing: std::cell::Cell<bool>,
    visible: std::cell::Cell<bool>,
}

fn menu_item(mtm: MainThreadMarker, target: &Target, title: &str, tag: NSInteger) -> Retained<NSMenuItem> {
    let item = NSMenuItem::new(mtm);
    item.setTitle(&NSString::from_str(title));
    // SAFETY: цель — наш же объект, он живёт в ITEMS до конца работы приложения.
    unsafe {
        item.setTarget(Some(target));
        item.setAction(Some(sel!(menuClicked:)));
    }
    item.setTag(tag);
    item
}

/// Показать значок в строке меню. Звать из главного потока, когда приложение уже запущено.
pub fn install(handler: impl Fn(Event) + 'static) {
    HANDLER.with(|h| *h.borrow_mut() = Some(Box::new(handler)));
    let Some(mtm) = MainThreadMarker::new() else {
        crate::config::log("трей: нет главного потока");
        return;
    };

    let target = Target::new(mtm);
    let status = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
    if let Some(button) = status.button(mtm) {
        // Готовый системный символ: выглядит родным и сам подстраивается под светлую и тёмную тему.
        let icon = NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &NSString::from_str("music.note"),
            Some(&NSString::from_str("Яндекс Музыка RS")),
        );
        if let Some(icon) = icon {
            icon.setTemplate(true);
            button.setImage(Some(&icon));
            button.setImagePosition(NSCellImagePosition::ImageOnly);
        }
    }

    let menu = NSMenu::new(mtm);
    let toggle = menu_item(mtm, &target, "Показать окно", TAG_TOGGLE);
    let play = menu_item(mtm, &target, "Играть", TAG_PLAY);
    menu.addItem(&toggle);
    menu.addItem(&menu_item(mtm, &target, "Предыдущий трек", TAG_PREV));
    menu.addItem(&menu_item(mtm, &target, "Следующий трек", TAG_NEXT));
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    menu.addItem(&menu_item(mtm, &target, "Выход", TAG_QUIT));
    // Автоподстановка состояний гасила бы пункты по своей логике, а нам нужен «Играть» всегда.
    menu.setAutoenablesItems(false);
    status.setMenu(Some(&menu));

    if !status.isVisible() {
        crate::config::log("трей: значок создан, но строка меню его не показывает");
    }
    ITEMS.with(|i| {
        *i.borrow_mut() = Some(Items {
            _status: status,
            _target: target,
            toggle,
            play,
            playing: std::cell::Cell::new(false),
            visible: std::cell::Cell::new(true),
        })
    });
}

/// Заголовки пунктов под текущее состояние плеера и окна.
pub fn sync(playing: bool, visible: bool) {
    ITEMS.with(|i| {
        if let Some(items) = i.borrow().as_ref() {
            if items.playing.get() != playing {
                items.playing.set(playing);
                items.play.setTitle(&NSString::from_str(if playing { "Пауза" } else { "Играть" }));
                items.play.setState(if playing { NSControlStateValueOn } else { NSControlStateValueOff });
            }
            if items.visible.get() != visible {
                items.visible.set(visible);
                items.toggle.setTitle(&NSString::from_str(if visible { "Скрыть окно" } else { "Показать окно" }));
            }
        }
    });
}

fn dispatch(e: Event) {
    HANDLER.with(|h| {
        if let Some(f) = h.borrow().as_ref() {
            f(e);
        }
    });
}

fn event_of(tag: NSInteger) -> Option<Event> {
    match tag {
        TAG_TOGGLE => Some(Event::Toggle),
        TAG_PLAY => Some(Event::PlayPause),
        TAG_PREV => Some(Event::Prev),
        TAG_NEXT => Some(Event::Next),
        TAG_QUIT => Some(Event::Quit),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_tags_map_to_events() {
        assert_eq!(event_of(TAG_TOGGLE), Some(Event::Toggle));
        assert_eq!(event_of(TAG_PLAY), Some(Event::PlayPause));
        assert_eq!(event_of(TAG_QUIT), Some(Event::Quit));
        assert_eq!(event_of(99), None);
    }
}
