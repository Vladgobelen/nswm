//! Протокол обмена сообщениями между `nswm` и клиентами (панелью).
//!
//! Формат фрейма: 4 байта (u32 big-endian) — длина сообщения,
//! затем `length` байт UTF-8 текста.
//!
//! Формат текста — плоский, вручную сериализованный (без serde):
//!
//! ```
//! type=windows
//! win=0x1a00006 pid=1234 class=firefox title=Mozilla Firefox exe=/usr/bin/firefox kind=native active=1 hidden=0
//! win=0x1a0000c pid=5678 class=wow.exe title=World of Warcraft exe=/home/user/Wow.exe kind=wine active=0 hidden=1
//! ```
//!
//! `type=windows` — снимок списка окон. Каждая строка `win=...` — одно окно.
//! `title` может содержать пробелы, поэтому разбираем по `key=value` парам,
//! где `value` — до следующего ` key=` или до конца строки.

use std::io::{self, Read, Write};

use crate::window::{BinaryKind, WindowInfo};

/// Маркер конца сообщения (не обязателен, но удобен для отладки).
pub const END: &str = "\n";

/// Сформировать текстовое сообщение `windows` из списка окон.
pub fn encode_windows(windows: &[WindowInfo]) -> String {
    let mut out = String::new();
    out.push_str("type=windows\n");
    for w in windows {
        out.push_str(&format!(
            "win=0x{:x} pid={} class={} instance={} title={} exe={} kind={} active={} hidden={}\n",
            w.id,
            w.pid.map(|p| p.to_string()).unwrap_or_else(|| "-".to_string()),
            esc(&w.class),
            esc(&w.instance),
            esc(&w.title),
            esc(&w.exe_path),
            w.kind.as_str(),
            if w.is_active { "1" } else { "0" },
            if w.is_hidden { "1" } else { "0" },
        ));
    }
    out
}

/// Записать сообщение в поток: 4 байта длины + текст.
pub fn write_frame<W: Write>(w: &mut W, msg: &str) -> io::Result<()> {
    let len = msg.len() as u32;
    w.write_all(&len.to_be_bytes())?;
    w.write_all(msg.as_bytes())?;
    w.flush()
}

/// Прочитать сообщение из потока (для клиентов — панели).
pub fn read_frame<R: Read>(r: &mut R) -> io::Result<Option<String>> {
    let mut len_buf = [0u8; 4];
    match r.read_exact(&mut len_buf) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > 16 * 1024 * 1024 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "message too large",
        ));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    Ok(Some(String::from_utf8_lossy(&buf).to_string()))
}

/// Экранировать пробелы и `=` в значениях, чтобы разбор `key=value`
/// был однозначным. Пробелы → `\s`, `=` → `\e`, `\` → `\\`.
fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            ' ' => out.push_str("\\s"),
            '=' => out.push_str("\\e"),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            _ => out.push(ch),
        }
    }
    out
}

/// Разэкранировать (для клиента).
#[allow(dead_code)]
fn unesc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.next() {
                Some('s') => out.push(' '),
                Some('e') => out.push('='),
                Some('\\') => out.push('\\'),
                Some('n') => out.push('\n'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(ch);
        }
    }
    out
}

/// Разобрать сообщение `windows` обратно в `Vec<WindowInfo>`.
/// Используется клиентом (панелью) — для теста и отладки.
#[allow(dead_code)]
pub fn decode_windows(msg: &str) -> Vec<WindowInfo> {
    let mut out = Vec::new();
    for line in msg.lines() {
        if !line.starts_with("win=") {
            continue;
        }
        let mut w = WindowInfo {
            id: 0,
            pid: None,
            title: String::new(),
            class: String::new(),
            instance: String::new(),
            exe_path: String::new(),
            kind: BinaryKind::Unknown,
            is_active: false,
            is_hidden: false,
        };
        // Первый токен: win=0x...
        let mut rest = line;
        if let Some(space) = rest.find(' ') {
            let first = &rest[..space];
            rest = &rest[space + 1..];
            if let Some(id_hex) = first.strip_prefix("win=0x") {
                w.id = u32::from_str_radix(id_hex, 16).unwrap_or(0);
            }
        }
        // Остальные: key=value, value до следующего " key=" (одна буква).
        // Простой разбор: разбить по " X=" где X — одна из букв ключей.
        // Мы знаем список ключей: pid, class, instance, title, exe, kind, active, hidden.
        let keys = ["pid", "class", "instance", "title", "exe", "kind", "active", "hidden"];
        let mut parts: Vec<(String, String)> = Vec::new();
        let mut current_key: Option<String> = None;
        let mut current_val = String::new();
        let mut chars = rest.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch == ' ' {
                // Может быть начало " key="
                let mut peek = chars.clone();
                let mut word = String::new();
                while let Some(&c) = peek.peek() {
                    if c == '=' { break; }
                    if c == ' ' { break; }
                    word.push(c);
                    peek.next();
                }
                if peek.peek() == Some(&'=') && keys.contains(&word.as_str()) {
                    if let Some(k) = current_key.take() {
                        parts.push((k, current_val.clone()));
                    }
                    current_val.clear();
                    current_key = Some(word);
                    // Съесть слово и '='
                    for _ in 0..word.len() { chars.next(); }
                    chars.next(); // '='
                    continue;
                }
            }
            if current_key.is_some() {
                current_val.push(ch);
            }
        }
        if let Some(k) = current_key.take() {
            parts.push((k, current_val));
        }

        for (k, v) in parts {
            match k.as_str() {
                "pid" => w.pid = v.parse().ok(),
                "class" => w.class = unesc(&v),
                "instance" => w.instance = unesc(&v),
                "title" => w.title = unesc(&v),
                "exe" => w.exe_path = unesc(&v),
                "kind" => {
                    w.kind = match v.as_str() {
                        "native" => BinaryKind::Native,
                        "wine" => BinaryKind::Wine,
                        "flatpak" => BinaryKind::Flatpak,
                        _ => BinaryKind::Unknown,
                    }
                }
                "active" => w.is_active = v == "1",
                "hidden" => w.is_hidden = v == "1",
                _ => {}
            }
        }
        out.push(w);
    }
    out
}