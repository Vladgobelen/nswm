mod ipc;
mod proc;
mod protocol;
mod window;
mod x11;

use std::os::fd::{AsRawFd, BorrowedFd};
use std::time::Duration;

use nix::poll::{poll, PollFd, PollFlags, PollTimeout};

use crate::ipc::{IpcServer, SOCKET_PATH};
use crate::protocol::encode_windows;
use crate::window::WindowInfo;
use crate::x11::X11;

fn main() {
    if let Err(e) = run() {
        eprintln!("nswm: fatal error: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    eprintln!("[nswm] starting");

    // 1. Подключаемся к X11 (читаем EWMH-свойства).
    let x11 = X11::connect()?;
    eprintln!("[nswm] connected to X11, root=0x{:x}", x11.root);

    // 2. Поднимаем Unix-сокет для панели.
    let mut server = IpcServer::bind(SOCKET_PATH)?;
    eprintln!("[nswm] listening on {}", SOCKET_PATH);

    // 3. Первый снимок — как только кто-то подключится, ему уйдёт актуальное.
    let mut prev_windows: Vec<WindowInfo> = Vec::new();
    let mut has_prev = false;

    let x11_fd = x11.conn.stream().as_raw_fd();
    let listener_fd = server_listener_fd(&server);

    loop {
        // Собираем fd-ы для poll.
        // x11_fd — ждём события X11.
        // listener_fd — ждём новых клиентов.
        let borrowed_x11 = unsafe { BorrowedFd::borrow_raw(x11_fd) };
        let borrowed_listener = unsafe { BorrowedFd::borrow_raw(listener_fd) };

        let mut fds = [
            PollFd::new(borrowed_x11, PollFlags::POLLIN),
            PollFd::new(borrowed_listener, PollFlags::POLLIN),
        ];

        // Ждём бесконечно (None), пока не придёт событие.
        // Если нужно таймаут — заменить на PollTimeout::try_from(Duration).
        let timeout = PollTimeout::NONE;

        match poll(&mut fds, timeout) {
            Ok(0) => continue, // таймаут (сейчас не бывает, но пусть будет)
            Ok(_) => {}
            Err(nix::errno::Errno::EINTR) => continue,
            Err(e) => return Err(format!("poll failed: {e}").into()),
        }

        // 1. Новые подключения?
        if fds[1].revents().contains(PollFlags::POLLIN) {
            server.accept_pending();
            // Новичку сразу шлём текущий снимок.
            if has_prev {
                let msg = encode_windows(&prev_windows);
                server.broadcast(&msg);
            }
        }

        // 2. События X11?
        if fds[0].revents().contains(PollFlags::POLLIN) {
            // Читаем все ожидающие события.
            loop {
                let ev = match x11.conn.poll_for_event() {
                    Ok(Some(ev)) => ev,
                    Ok(None) => break,
                    Err(e) => return Err(format!("x11 poll_for_event: {e}").into()),
                };

                // Нас интересуют только PropertyNotify на корневом окне
                // с атомами _NET_CLIENT_LIST или _NET_ACTIVE_WINDOW.
                if let x11rb::protocol::Event::PropertyNotify(e) = ev {
                    if e.window == x11.root
                        && (e.atom == x11.atoms.net_client_list
                            || e.atom == x11.atoms.net_active_window)
                    {
                        let wins = x11.build_windows();
                        if has_prev && !windows_changed(&prev_windows, &wins) {
                            // Ничего не изменилось (например, _NET_ACTIVE_WINDOW
                            // указал на то же окно) — не спамим.
                            continue;
                        }
                        prev_windows = wins;
                        has_prev = true;

                        if server.clients_count() > 0 {
                            let msg = encode_windows(&prev_windows);
                            server.broadcast(&msg);
                        }
                    }
                }
            }
        }
    }
}

/// Получить fd слушателя из `IpcServer`. `IpcServer::listener` приватный,
/// поэтому берём через `AsRawFd` на самом сервере.
fn server_listener_fd(server: &IpcServer) -> std::os::fd::RawFd {
    server.as_raw_fd()
}

/// Сравнить два снимка окон. Возвращает `true`, если что-то изменилось
/// (набор id, активное окно, свёрнутость, заголовок, class, exe_path).
fn windows_changed(a: &[WindowInfo], b: &[WindowInfo]) -> bool {
    if a.len() != b.len() {
        return true;
    }
    for (x, y) in a.iter().zip(b.iter()) {
        if x.id != y.id
            || x.is_active != y.is_active
            || x.is_hidden != y.is_hidden
            || x.title != y.title
            || x.class != y.class
            || x.exe_path != y.exe_path
        {
            return true;
        }
    }
    false
}