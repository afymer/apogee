use std::sync::Arc;

use gpui::{App, Global, Hsla, SharedString, rgb};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeMode {
    Light,
    Dark,
    Night,
}
impl ThemeMode {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Light => "Light",
            Self::Dark => "Dark",
            Self::Night => "Night",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Theme {
    pub mode: ThemeMode,
    pub name: SharedString,

    pub background: Hsla,

    pub text: Hsla,
    pub text_muted: Hsla,
}

impl Theme {
    pub fn for_mode(mode: ThemeMode) -> Self {
        match mode {
            ThemeMode::Light => Self::light(),
            ThemeMode::Dark => Self::dark(),
            ThemeMode::Night => Self::night(),
        }
    }

    pub fn light() -> Self {
        Self {
            mode: ThemeMode::Light,
            name: "Light".into(),
            background: rgb(0xffffff).into(),
            text: rgb(0x09090b).into(),
            text_muted: rgb(0x71717a).into(),
        }
    }
    pub fn dark() -> Self {
        Self {
            mode: ThemeMode::Dark,
            name: "Dark".into(),
            background: rgb(0x18181b).into(),
            text: rgb(0xf4f4f5).into(),
            text_muted: rgb(0xa1a1aa).into(),
        }
    }
    pub fn night() -> Self {
        Self {
            mode: ThemeMode::Night,
            name: "Night".into(),

            background: rgb(0x000000).into(),
            text: rgb(0xfafafa).into(),
            text_muted: rgb(0x85858e).into(),
        }
    }
}

struct GlobalTheme(Arc<Theme>);
impl Global for GlobalTheme {}

pub trait ActiveTheme {
    fn theme(&self) -> &Theme;
}

impl ActiveTheme for App {
    fn theme(&self) -> &Theme {
        &self.global::<GlobalTheme>().0
    }
}

pub fn init(cx: &mut App, initial_mode: ThemeMode) {
    let theme = Arc::new(Theme::for_mode(initial_mode));
    cx.set_global(GlobalTheme(theme));
}

pub fn switch_theme(mode: ThemeMode, cx: &mut App) {
    let new_theme = Arc::new(Theme::for_mode(mode));
    cx.set_global(GlobalTheme(new_theme));
    cx.refresh_windows();
}
