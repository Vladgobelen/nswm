//! Работа с `/proc`: определение пути к бинарнику и типа приложения.
//!
//! - Нативные приложения: `readlink /proc/<pid>/exe`.
//! - Wine/Proton: `/proc/<pid>/exe` указывает на `wine`/`wine64-preloader`,
//!   а реальный `.exe` виден только в `/proc/<pid>/maps` (самый большой маппинг).
//! - Flatpak: `/proc/<pid>/exe` указывает на `bwrap`.
//!
//! `proc_exe` возвращает путь и `BinaryKind`. Если что-то пошло не так —
//! (`None`, `BinaryKind::Unknown`), вызывающий код делает fallback на `WM_CLASS`.

use std::fs;
use std::io::Read;

use crate::window::BinaryKind;

/// Определить путь к бинарнику по pid.
pub fn resolve_exe(pid: u32) -> (String, BinaryKind) {
    let exe_link = format!("/proc/{pid}/exe");
    let exe_path = match fs::read_link(&exe_link) {
        Ok(p) => p.to_string_lossy().to_string(),
        Err(_) => return (String::new(), BinaryKind::Unknown),
    };

    let base = exe_path.rsplit('/').next().unwrap_or("");

    // Wine / Proton: exe → wine, wine64, wine-preloader, wine64-preloader.
    if matches!(
        base,
        "wine" | "wine64" | "wine-preloader" | "wine64-preloader"
    ) {
        if let Some(w) = wine_exe_from_maps(pid) {
            return (w, BinaryKind::Wine);
        }
        // Не нашли .exe в maps — возвращаем как есть.
        return (exe_path, BinaryKind::Wine);
    }

    // Flatpak: exe → bwrap. Реальный путь внутри песочницы — сложнее,
    // для прототипа возвращаем bwrap.
    if base == "bwrap" {
        return (exe_path, BinaryKind::Flatpak);
    }

    (exe_path, BinaryKind::Native)
}

/// Для Wine: найти самый большой `.exe`-маппинг в `/proc/<pid>/maps`.
/// Это и есть основной бинарник игры/приложения.
fn wine_exe_from_maps(pid: u32) -> Option<String> {
    let maps_path = format!("/proc/{pid}/maps");
    let mut f = fs::File::open(&maps_path).ok()?;
    let mut content = String::new();
    f.read_to_string(&mut content).ok()?;

    let mut best: Option<(u64, String)> = None;

    for line in content.lines() {
        // Формат: "addr-addr perms offset dev inode pathname"
        let mut it = line.split_whitespace();
        let range = it.next()?;
        let _perms = it.next();
        let _offset = it.next();
        let _dev = it.next();
        let _inode = it.next();
        let path = match it.next() {
            Some(p) => p,
            None => continue, // анонимный маппинг
        };

        if !path.to_lowercase().ends_with(".exe") {
            continue;
        }

        let mut parts = range.split('-');
        let a = parts.next().and_then(|s| u64::from_str_radix(s, 16).ok());
        let b = parts.next().and_then(|s| u64::from_str_radix(s, 16).ok());
        if let (Some(a), Some(b)) = (a, b) {
            let size = b.saturating_sub(a);
            if best.as_ref().map(|(s, _)| size > *s).unwrap_or(true) {
                best = Some((size, path.to_string()));
            }
        }
    }

    best.map(|(_, p)| p)
}

/// Прочитать `cmdline` процесса (аргументы через `\0`).
/// Пока не используется, но пригодится, если два окна одного `exe_path`
/// нужно различить по аргументам (например, два клиента WoW из одной папки).
#[allow(dead_code)]
pub fn proc_cmdline(pid: u32) -> Option<Vec<String>> {
    let s = fs::read_to_string(format!("/proc/{pid}/cmdline")).ok()?;
    let args: Vec<String> = s
        .split('\0')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect();
    if args.is_empty() { None } else { Some(args) }
}