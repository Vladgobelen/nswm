//! Unix-сокет-сервер для `nswm`.
//!
//! Клиенты (панель, скрипты) подключаются к `/tmp/nswm.sock`.
//! Сервер принимает подключения, хранит список живых клиентов и умеет
//! рассылать им сообщения (`broadcast`).
//!
//! Удаление сокета при старте: если файл остался от прошлого запуска
//! (например, `nswm` упал), он удаляется. Иначе `bind` вернёт `EADDRINUSE`.
//!
//! Обработка отвалившихся клиентов: при `write` в мёртвый сокет
//! (EPIPE / BrokenPipe) клиент удаляется из списка.

use std::io::{self, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};

use crate::protocol;

/// Путь к сокету по умолчанию.
pub const SOCKET_PATH: &str = "/tmp/nswm.sock";

pub struct IpcServer {
    listener: UnixListener,
    path: PathBuf,
    clients: Vec<UnixStream>,
}

impl IpcServer {
    /// Создать сервер. Если файл сокета уже существует — удалить.
    pub fn bind<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        if path.exists() {
            let _ = std::fs::remove_file(&path);
        }
        let listener = UnixListener::bind(&path)?;
        listener.set_nonblocking(true)?;
        Ok(Self {
            listener,
            path,
            clients: Vec::new(),
        })
    }

    /// Принять все ожидающие подключения (неблокирующий).
    pub fn accept_pending(&mut self) {
        loop {
            match self.listener.accept() {
                Ok((stream, _addr)) => {
                    let _ = stream.set_nonblocking(true);
                    eprintln!("[nswm] client connected");
                    self.clients.push(stream);
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) => {
                    eprintln!("[nswm] accept error: {e}");
                    break;
                }
            }
        }
    }

    /// Разослать сообщение всем клиентам. Отвалившиеся удаляются.
    pub fn broadcast(&mut self, msg: &str) {
        let mut dead = Vec::new();
        for (i, client) in self.clients.iter_mut().enumerate() {
            if let Err(e) = protocol::write_frame(client, msg) {
                match e.kind() {
                    io::ErrorKind::BrokenPipe
                    | io::ErrorKind::ConnectionReset
                    | io::ErrorKind::WriteZero => {
                        dead.push(i);
                    }
                    _ => {
                        eprintln!("[nswm] write error to client {i}: {e}");
                        dead.push(i);
                    }
                }
            }
        }
        // Удаляем в обратном порядке, чтобы индексы не сдвинулись.
        for i in dead.into_iter().rev() {
            let _ = self.clients.remove(i);
            eprintln!("[nswm] client disconnected");
        }
    }

    /// Количество живых клиентов.
    pub fn clients_count(&self) -> usize {
        self.clients.len()
    }
}

impl Drop for IpcServer {
    fn drop(&mut self) {
        // Удаляем файл сокета при выходе.
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Клиент для подключения к `nswm` (используется панелью).
#[allow(dead_code)]
pub struct IpcClient {
    stream: UnixStream,
}

#[allow(dead_code)]
impl IpcClient {
    pub fn connect<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        let stream = UnixStream::connect(path)?;
        Ok(Self { stream })
    }

    /// Прочитать следующее сообщение (блокирующий вызов).
    pub fn recv(&mut self) -> io::Result<Option<String>> {
        protocol::read_frame(&mut self.stream)
    }

    /// Отправить сообщение (например, команду "activate window").
    pub fn send(&mut self, msg: &str) -> io::Result<()> {
        let mut s = &self.stream;
        protocol::write_frame(&mut s, msg)
    }
}