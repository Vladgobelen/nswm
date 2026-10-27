//! Структура описания окна, которую `nswm` рассылает клиентам (панели).
//!
//! Поля минимальны и стабильны. Всё, что нужно таскбару / панели,
//! чтобы нарисовать иконку, обработать клик, сгруппировать окна:
//!
//! - `id` — X11 window id.
//! - `pid` — `_NET_WM_PID` (может отсутствовать).
//! - `title` — `_NET_WM_NAME` (UTF-8) или `WM_NAME`.
//! - `class` — `WM_CLASS` (class).
//! - `instance` — `WM_CLASS` (instance).
//! - `exe_path` — путь к бинарнику: `/proc/<pid>/exe` (native) или
//!   `.exe` из `/proc/<pid>/maps` (wine). Пусто, если не удалось.
//! - `kind` — тип бинарника: `native` | `wine` | `flatpak` | `unknown`.
//! - `is_active` — сейчас в фокусе (`_NET_ACTIVE_WINDOW`).
//! - `is_hidden` — свёрнуто (`_NET_WM_STATE_HIDDEN`).
//!
//! `exe_path` — стабильный ключ «наш ID»: переживает перезапуск окна.
//! `class` — fallback, если `exe_path` недоступен.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryKind {
    Native,
    Wine,
    Flatpak,
    Unknown,
}

impl BinaryKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            BinaryKind::Native => "native",
            BinaryKind::Wine => "wine",
            BinaryKind::Flatpak => "flatpak",
            BinaryKind::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone)]
pub struct WindowInfo {
    pub id: u32,
    pub pid: Option<u32>,
    pub title: String,
    pub class: String,
    pub instance: String,
    pub exe_path: String,
    pub kind: BinaryKind,
    pub is_active: bool,
    pub is_hidden: bool,
}

impl WindowInfo {
    /// Стабильный ключ для группировки (наш «AppId»).
    /// Сначала `exe_path`, потом `class`, потом `id` (fallback).
    pub fn app_key(&self) -> String {
        if !self.exe_path.is_empty() {
            self.exe_path.clone()
        } else if !self.class.is_empty() {
            format!("class:{}", self.class)
        } else {
            format!("id:{}", self.id)
        }
    }

    /// Человекочитаемый лейбл для UI.
    pub fn label(&self) -> String {
        if !self.class.is_empty() {
            self.class.clone()
        } else if !self.title.is_empty() {
            self.title.clone()
        } else {
            format!("0x{:x}", self.id)
        }
    }
}