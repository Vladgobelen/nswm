//! Работа с X11: подключение, чтение EWMH-свойств, сборка списка окон.
//!
//! `nswm` не является WM. Он читает то, что публикует настоящий WM
//! (Openbox, KWin, ...) — `_NET_CLIENT_LIST`, `_NET_ACTIVE_WINDOW`.
//! В будущем, когда `nswm` станет WM, он будет *писать* эти свойства,
//! но логика сбора информации (`build_windows`) останется той же.

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    AtomEnum, ConnectionExt as _, EventMask, Window,
};
use x11rb::rust_connection::RustConnection;

use crate::proc;
use crate::window::{BinaryKind, WindowInfo};

#[derive(Debug, Clone, Copy)]
pub struct Atoms {
    pub net_client_list: u32,
    pub net_active_window: u32,
    pub net_wm_pid: u32,
    pub net_wm_name: u32,
    pub wm_name: u32,
    pub wm_class: u32,
    pub net_wm_state: u32,
    pub net_wm_state_hidden: u32,
    pub net_wm_state_skip_taskbar: u32,
    pub net_wm_window_type: u32,
    pub net_wm_window_type_normal: u32,
}

pub struct X11 {
    pub conn: RustConnection,
    pub screen_num: usize,
    pub root: Window,
    pub atoms: Atoms,
}

impl X11 {
    pub fn connect() -> Result<Self, Box<dyn std::error::Error>> {
        let (conn, screen_num) = x11rb::connect(None)?;
        let root = conn.setup().roots[screen_num].root;

        // Подписываемся на изменения свойств корневого окна.
        // Это позволит нам ловить _NET_CLIENT_LIST и _NET_ACTIVE_WINDOW.
        conn.change_window_attributes(
            root,
            &x11rb::protocol::xproto::ChangeWindowAttributesAux::new()
                .event_mask(EventMask::PROPERTY_CHANGE),
        )?;
        conn.flush()?;

        let intern = |name: &[u8]| -> Result<u32, Box<dyn std::error::Error>> {
            Ok(conn.intern_atom(false, name)?.reply()?.atom)
        };

        let atoms = Atoms {
            net_client_list: intern(b"_NET_CLIENT_LIST")?,
            net_active_window: intern(b"_NET_ACTIVE_WINDOW")?,
            net_wm_pid: intern(b"_NET_WM_PID")?,
            net_wm_name: intern(b"_NET_WM_NAME")?,
            wm_name: intern(b"WM_NAME")?,
            wm_class: intern(b"WM_CLASS")?,
            net_wm_state: intern(b"_NET_WM_STATE")?,
            net_wm_state_hidden: intern(b"_NET_WM_STATE_HIDDEN")?,
            net_wm_state_skip_taskbar: intern(b"_NET_WM_STATE_SKIP_TASKBAR")?,
            net_wm_window_type: intern(b"_NET_WM_WINDOW_TYPE")?,
            net_wm_window_type_normal: intern(b"_NET_WM_WINDOW_TYPE_NORMAL")?,
        };

        Ok(Self { conn, screen_num, root, atoms })
    }

    /// Прочитать одно свойство типа CARDINAL (u32).
    fn prop_u32(&self, win: Window, atom: u32) -> Option<u32> {
        let r = self.conn
            .get_property(false, win, atom, AtomEnum::CARDINAL, 0, 1)
            .ok()?
            .reply()
            .ok()?;
        if r.value_len == 0 || r.value.len() < 4 {
            return None;
        }
        Some(u32::from_ne_bytes([r.value[0], r.value[1], r.value[2], r.value[3]]))
    }

    /// Прочитать свойство как строку.
    fn prop_string(&self, win: Window, atom: u32) -> Option<String> {
        let r = self.conn
            .get_property(false, win, atom, AtomEnum::ANY, 0, 1024)
            .ok()?
            .reply()
            .ok()?;
        if r.value_len == 0 {
            return None;
        }
        let s = String::from_utf8_lossy(&r.value).to_string();
        let s = s.trim_matches('\0').trim().to_string();
        if s.is_empty() { None } else { Some(s) }
    }

    /// Прочитать WM_CLASS (две строки: instance, class).
    fn prop_wm_class(&self, win: Window) -> (String, String) {
        let r = self.conn
            .get_property(false, win, self.atoms.wm_class, AtomEnum::STRING, 0, 64)
            .ok()
            .and_then(|c| c.reply().ok());
        
        if let Some(r) = r {
            if r.value_len > 0 {
                let parts: Vec<String> = r.value
                    .split(|&b| b == 0)
                    .filter(|s| !s.is_empty())
                    .map(|s| String::from_utf8_lossy(s).to_string())
                    .collect();
                let instance = parts.get(0).cloned().unwrap_or_default();
                let class = parts.get(1).cloned().unwrap_or_default();
                return (instance, class);
            }
        }
        (String::new(), String::new())
    }

    /// Прочитать массив ATOM (например, _NET_WM_STATE или _NET_WM_WINDOW_TYPE).
    fn prop_atoms(&self, win: Window, atom: u32) -> Vec<u32> {
        let r = self.conn
            .get_property(false, win, atom, AtomEnum::ATOM, 0, 16)
            .ok()
            .and_then(|c| c.reply().ok());
        
        if let Some(r) = r {
            r.value32().map(|it| it.collect()).unwrap_or_default()
        } else {
            Vec::new()
        }
    }

    /// Проверить, является ли окно обычным приложением (не панелью, не рабочим столом).
    fn is_normal_window(&self, win: Window) -> bool {
        let states = self.prop_atoms(win, self.atoms.net_wm_state);
        if states.contains(&self.atoms.net_wm_state_skip_taskbar) {
            return false;
        }

        let types = self.prop_atoms(win, self.atoms.net_wm_window_type);
        if types.is_empty() {
            return true; // Отсутствие типа = NORMAL по EWMH
        }
        
        types.contains(&self.atoms.net_wm_window_type_normal)
    }

    /// Собрать актуальный список окон.
    pub fn build_windows(&self) -> Vec<WindowInfo> {
        // 1. Читаем активное окно
        let active_win = self.prop_u32(self.root, self.atoms.net_active_window).unwrap_or(0);

        // 2. Читаем список окон (WINDOW = XID)
        let client_list = match self.conn
            .get_property(false, self.root, self.atoms.net_client_list, AtomEnum::WINDOW, 0, 1024)
            .ok()
            .and_then(|c| c.reply().ok())
        {
            Some(r) => r.value32().map(|it| it.collect::<Vec<Window>>()).unwrap_or_default(),
            None => Vec::new(),
        };

        let mut windows = Vec::new();

        for win in client_list {
            if !self.is_normal_window(win) {
                continue;
            }

            // Основные данные
            let pid = self.prop_u32(win, self.atoms.net_wm_pid);
            let title = self.prop_string(win, self.atoms.net_wm_name)
                .or_else(|| self.prop_string(win, self.atoms.wm_name))
                .unwrap_or_default();
            
            let (instance, class) = self.prop_wm_class(win);

            // Пропускаем пустые (нет ни title, ни class)
            if title.is_empty() && class.is_empty() {
                continue;
            }

            // Обогащение из /proc
            let (exe_path, kind) = match pid {
                Some(p) => proc::resolve_exe(p),
                None => (String::new(), BinaryKind::Unknown),
            };

            // Состояние (свернуто ли)
            let states = self.prop_atoms(win, self.atoms.net_wm_state);
            let is_hidden = states.contains(&self.atoms.net_wm_state_hidden);
            let is_active = win == active_win;

            windows.push(WindowInfo {
                id: win,
                pid,
                title,
                class,
                instance,
                exe_path,
                kind,
                is_active,
                is_hidden,
            });
        }

        windows
    }
}