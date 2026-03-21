use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use embers_client::{
    ConfigManager, ConfiguredClient, KeyEvent, MuxClient, RenderGrid, SocketTransport,
};
use embers_core::{MuxError, Result, SessionId, Size};
use embers_protocol::{BufferRequest, ClientMessage, ServerResponse, SessionRequest};
use tokio::sync::mpsc;

const DEFAULT_SESSION_NAME: &str = "main";
const KEY_SEQUENCE_TIMEOUT: Duration = Duration::from_millis(15);
const KEY_SEQUENCE_CONTINUATION_TIMEOUT: Duration = Duration::from_millis(2);
const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(20);

pub async fn run(
    socket_path: PathBuf,
    target: Option<String>,
    config_path: Option<PathBuf>,
) -> Result<()> {
    let mut client = MuxClient::connect(&socket_path).await?;
    client.subscribe(None).await?;
    let requested_target = target;
    let mut session_id = ensure_session_ready(&mut client, requested_target.as_deref()).await?;
    let config = ConfigManager::from_process(config_path)
        .map_err(|error| MuxError::invalid_input(error.to_string()))?;
    let mut configured = ConfiguredClient::new(client, config);

    let terminal = TerminalGuard::enter()?;
    let (input_tx, mut input_rx) = mpsc::unbounded_channel();
    let _input_thread = spawn_input_thread(input_tx)?;

    let mut terminal_size = terminal.size()?;
    let mut dirty = true;
    loop {
        if dirty {
            if !configured
                .client()
                .state()
                .sessions
                .contains_key(&session_id)
            {
                session_id =
                    ensure_session_ready(configured.client_mut(), requested_target.as_deref())
                        .await?;
            }
            terminal_size = terminal.size()?;
            let viewport = content_viewport(terminal_size);
            let grid = configured.render_session(session_id, viewport).await?;
            let status = status_line(&configured, session_id, &socket_path);
            terminal.render(&grid, terminal_size, Some(&status))?;
            dirty = false;
        }

        loop {
            match input_rx.try_recv() {
                Ok(TerminalEvent::Key(KeyEvent::Ctrl('q'))) => return Ok(()),
                Ok(TerminalEvent::Key(key)) => {
                    let viewport = content_viewport(terminal_size);
                    configured.handle_key(session_id, viewport, key).await?;
                    dirty = true;
                }
                Ok(TerminalEvent::InputClosed) => return Ok(()),
                Ok(TerminalEvent::InputError(message)) => {
                    return Err(MuxError::transport(message));
                }
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => return Ok(()),
            }
        }

        let next_size = terminal.size()?;
        if next_size != terminal_size {
            terminal_size = next_size;
            dirty = true;
            continue;
        }

        match tokio::time::timeout(EVENT_POLL_INTERVAL, configured.process_next_event()).await {
            Ok(result) => {
                result?;
                dirty = true;
            }
            Err(_) => {
                continue;
            }
        }
    }
}

async fn ensure_session_ready(
    client: &mut MuxClient<SocketTransport>,
    target: Option<&str>,
) -> Result<SessionId> {
    client.resync_all_sessions().await?;
    let session_id = match select_session_id(client, target)? {
        Some(session_id) => session_id,
        None => create_session(client, target.unwrap_or(DEFAULT_SESSION_NAME)).await?,
    };
    ensure_root_window(client, session_id).await?;
    client.resync_session(session_id).await?;
    Ok(session_id)
}

fn select_session_id(
    client: &MuxClient<SocketTransport>,
    target: Option<&str>,
) -> Result<Option<SessionId>> {
    if client.state().sessions.is_empty() {
        return Ok(None);
    }

    if let Some(target) = target {
        return client
            .state()
            .sessions
            .values()
            .find(|session| session.name == target)
            .map(|session| Some(session.id))
            .ok_or_else(|| MuxError::not_found(format!("session '{target}' was not found")));
    }

    Ok(client
        .state()
        .sessions
        .values()
        .max_by_key(|session| session.id.0)
        .map(|session| session.id))
}

async fn create_session(client: &mut MuxClient<SocketTransport>, name: &str) -> Result<SessionId> {
    let response = client
        .request_message(ClientMessage::Session(SessionRequest::Create {
            request_id: client.next_request_id(),
            name: name.to_owned(),
        }))
        .await?;
    match response {
        ServerResponse::SessionSnapshot(response) => {
            let session_id = response.snapshot.session.id;
            client.state_mut().apply_session_snapshot(response.snapshot);
            Ok(session_id)
        }
        other => Err(MuxError::protocol(format!(
            "expected session snapshot response, got {other:?}"
        ))),
    }
}

async fn ensure_root_window(
    client: &mut MuxClient<SocketTransport>,
    session_id: SessionId,
) -> Result<()> {
    client.resync_session(session_id).await?;
    if session_has_root_window(client, session_id)? {
        return Ok(());
    }

    let command = default_shell_command();
    let title = default_title(&command, "shell");
    let buffer_id = create_buffer(client, &command, &title).await?;
    let response = client
        .request_message(ClientMessage::Session(SessionRequest::AddRootTab {
            request_id: client.next_request_id(),
            session_id,
            title,
            buffer_id: Some(buffer_id),
            child_node_id: None,
        }))
        .await?;
    match response {
        ServerResponse::SessionSnapshot(response) => {
            client.state_mut().apply_session_snapshot(response.snapshot);
            Ok(())
        }
        other => Err(MuxError::protocol(format!(
            "expected session snapshot response, got {other:?}"
        ))),
    }
}

fn session_has_root_window(
    client: &MuxClient<SocketTransport>,
    session_id: SessionId,
) -> Result<bool> {
    let session = client
        .state()
        .sessions
        .get(&session_id)
        .ok_or_else(|| MuxError::not_found(format!("session {session_id} is not cached")))?;
    let root = client
        .state()
        .nodes
        .get(&session.root_node_id)
        .ok_or_else(|| {
            MuxError::not_found(format!("node {} is not cached", session.root_node_id))
        })?;
    let tabs = root
        .tabs
        .as_ref();
    Ok(tabs.map_or(true, |tabs| !tabs.tabs.is_empty()))
}

async fn create_buffer(
    client: &mut MuxClient<SocketTransport>,
    command: &[String],
    title: &str,
) -> Result<embers_core::BufferId> {
    let response = client
        .request_message(ClientMessage::Buffer(BufferRequest::Create {
            request_id: client.next_request_id(),
            title: Some(title.to_owned()),
            command: command.to_vec(),
            cwd: None,
            env: Default::default(),
        }))
        .await?;
    match response {
        ServerResponse::Buffer(response) => Ok(response.buffer.id),
        other => Err(MuxError::protocol(format!(
            "expected buffer response, got {other:?}"
        ))),
    }
}

fn default_shell_command() -> Vec<String> {
    vec![std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned())]
}

fn default_title(command: &[String], fallback: &str) -> String {
    command
        .first()
        .and_then(|value| {
            Path::new(value)
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| fallback.to_owned())
}

fn content_viewport(size: Size) -> Size {
    if size.height > 1 {
        Size {
            width: size.width,
            height: size.height - 1,
        }
    } else {
        size
    }
}

fn status_line(
    configured: &ConfiguredClient<SocketTransport>,
    session_id: SessionId,
    socket_path: &Path,
) -> String {
    let session_name = configured
        .client()
        .state()
        .sessions
        .get(&session_id)
        .map(|session| session.name.as_str())
        .unwrap_or("<missing>");
    match configured.notifications().last() {
        Some(message) => format!("[{session_name}] {message}"),
        None => format!("[{session_name}] {}  ctrl-q quit", socket_path.display()),
    }
}

enum TerminalEvent {
    Key(KeyEvent),
    InputClosed,
    InputError(String),
}

fn spawn_input_thread(
    tx: mpsc::UnboundedSender<TerminalEvent>,
) -> Result<std::thread::JoinHandle<()>> {
    thread::Builder::new()
        .name("embers-input".to_owned())
        .spawn(move || {
            let stdin = io::stdin();
            // Keep the stdin lock alive for the full read loop so the raw fd stays valid.
            let stdin_lock = stdin.lock();
            let fd = stdin_lock.as_raw_fd();
            let _stdin_lock = stdin_lock;
            loop {
                match read_key_event(fd) {
                    Ok(Some(key)) => {
                        if tx.send(TerminalEvent::Key(key)).is_err() {
                            break;
                        }
                    }
                    Ok(None) => {
                        let _ = tx.send(TerminalEvent::InputClosed);
                        break;
                    }
                    Err(error) => {
                        let _ = tx.send(TerminalEvent::InputError(error.to_string()));
                        break;
                    }
                }
            }
        })
        .map_err(|error| MuxError::internal(format!("failed to spawn input thread: {error}")))
}

fn read_key_event(fd: libc::c_int) -> Result<Option<KeyEvent>> {
    let Some(first) = read_byte(fd)? else {
        return Ok(None);
    };
    let event = match first {
        b'\r' | b'\n' => KeyEvent::Enter,
        b'\t' => KeyEvent::Tab,
        0x7f | 0x08 => KeyEvent::Backspace,
        0x1b => read_escape_key(fd)?,
        0x01..=0x1a => KeyEvent::Ctrl(char::from(b'a' + first - 1)),
        0x20..=0x7e => KeyEvent::Char(char::from(first)),
        other => decode_utf8_key(fd, other)?,
    };
    Ok(Some(event))
}

fn read_escape_key(fd: libc::c_int) -> Result<KeyEvent> {
    let Some(next) = read_optional_byte(fd, KEY_SEQUENCE_TIMEOUT)? else {
        return Ok(KeyEvent::Escape);
    };
    let mut bytes = vec![0x1b, next];
    while let Some(extra) = read_optional_byte(fd, KEY_SEQUENCE_CONTINUATION_TIMEOUT)? {
        bytes.push(extra);
    }
    if bytes.len() == 2 && bytes[1].is_ascii() {
        return Ok(KeyEvent::Alt(char::from(bytes[1])));
    }
    Ok(KeyEvent::Bytes(bytes))
}

fn decode_utf8_key(fd: libc::c_int, first: u8) -> Result<KeyEvent> {
    let width = utf8_width(first);
    if width <= 1 {
        return Ok(KeyEvent::Bytes(vec![first]));
    }

    let mut bytes = vec![first];
    for _ in 1..width {
        let Some(next) = read_byte(fd)? else {
            return Ok(KeyEvent::Bytes(bytes));
        };
        bytes.push(next);
    }

    match std::str::from_utf8(&bytes)
        .ok()
        .and_then(|text| text.chars().next())
    {
        Some(ch) => Ok(KeyEvent::Char(ch)),
        None => Ok(KeyEvent::Bytes(bytes)),
    }
}

fn utf8_width(first: u8) -> usize {
    match first {
        0x00..=0x7f => 1,
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf7 => 4,
        _ => 0,
    }
}

fn read_optional_byte(fd: libc::c_int, timeout: Duration) -> Result<Option<u8>> {
    if poll_fd(fd, timeout)? {
        read_byte(fd)
    } else {
        Ok(None)
    }
}

fn poll_fd(fd: libc::c_int, timeout: Duration) -> Result<bool> {
    let timeout_ms = timeout.as_millis().min(i32::MAX as u128) as i32;
    let mut poll_fd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    loop {
        // SAFETY: poll_fd points to a valid pollfd on the stack and we pass a valid count.
        let result = unsafe { libc::poll(&mut poll_fd, 1, timeout_ms) };
        if result == 0 {
            return Ok(false);
        }
        if result > 0 {
            return Ok((poll_fd.revents & libc::POLLIN) != 0);
        }
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::Interrupted {
            continue;
        }
        return Err(error.into());
    }
}

fn read_byte(fd: libc::c_int) -> Result<Option<u8>> {
    let mut byte = 0_u8;
    loop {
        // SAFETY: we pass a valid fd and a writable pointer to a single-byte buffer.
        let result = unsafe { libc::read(fd, (&mut byte as *mut u8).cast(), 1) };
        if result == 0 {
            return Ok(None);
        }
        if result > 0 {
            return Ok(Some(byte));
        }
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::Interrupted {
            continue;
        }
        return Err(error.into());
    }
}

struct TerminalGuard {
    input_fd: libc::c_int,
    original_mode: libc::termios,
}

impl TerminalGuard {
    fn enter() -> Result<Self> {
        let input_fd = io::stdin().as_raw_fd();
        let output_fd = io::stdout().as_raw_fd();
        if !is_tty(input_fd) || !is_tty(output_fd) {
            return Err(MuxError::invalid_input(
                "interactive embers client requires a TTY on stdin/stdout",
            ));
        }

        let original_mode = terminal_mode(input_fd)?;
        let mut raw_mode = original_mode;
        raw_mode.c_iflag &= !(libc::BRKINT | libc::ICRNL | libc::INPCK | libc::ISTRIP | libc::IXON);
        raw_mode.c_oflag &= !libc::OPOST;
        raw_mode.c_cflag |= libc::CS8;
        raw_mode.c_lflag &= !(libc::ECHO | libc::ICANON | libc::IEXTEN | libc::ISIG);
        raw_mode.c_cc[libc::VMIN] = 1;
        raw_mode.c_cc[libc::VTIME] = 0;
        set_terminal_mode(input_fd, &raw_mode)?;

        let mut stdout = io::stdout();
        write!(stdout, "\x1b[?1049h\x1b[?25l\x1b[2J\x1b[H")?;
        stdout.flush()?;

        Ok(Self {
            input_fd,
            original_mode,
        })
    }

    fn size(&self) -> Result<Size> {
        terminal_size(io::stdout().as_raw_fd())
    }

    fn render(&self, grid: &RenderGrid, terminal_size: Size, status: Option<&str>) -> Result<()> {
        let mut stdout = io::stdout();
        write!(stdout, "\x1b[H")?;
        for line in grid.lines() {
            write!(stdout, "{line}\x1b[K\r\n")?;
        }

        if terminal_size.height > grid.height() {
            let status = fit_width(status.unwrap_or_default(), terminal_size.width);
            write!(stdout, "\x1b[7m{status}\x1b[0m\x1b[K")?;
        }

        write!(stdout, "\x1b[J")?;
        stdout.flush()?;
        Ok(())
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = set_terminal_mode(self.input_fd, &self.original_mode);
        let mut stdout = io::stdout();
        let _ = write!(stdout, "\x1b[0m\x1b[?25h\x1b[?1049l");
        let _ = stdout.flush();
    }
}

fn is_tty(fd: libc::c_int) -> bool {
    // SAFETY: isatty only inspects the fd and has no additional invariants.
    unsafe { libc::isatty(fd) == 1 }
}

fn terminal_mode(fd: libc::c_int) -> Result<libc::termios> {
    // SAFETY: termios is a plain old data struct and zero initialization is valid.
    let mut mode = unsafe { std::mem::zeroed::<libc::termios>() };
    // SAFETY: tcgetattr writes to the provided termios pointer when fd is valid.
    if unsafe { libc::tcgetattr(fd, &mut mode) } == -1 {
        return Err(io::Error::last_os_error().into());
    }
    Ok(mode)
}

fn set_terminal_mode(fd: libc::c_int, mode: &libc::termios) -> Result<()> {
    // SAFETY: tcsetattr reads the provided termios pointer and applies it to the valid fd.
    if unsafe { libc::tcsetattr(fd, libc::TCSAFLUSH, mode) } == -1 {
        return Err(io::Error::last_os_error().into());
    }
    Ok(())
}

fn terminal_size(fd: libc::c_int) -> Result<Size> {
    // SAFETY: winsize is POD and zero initialization is valid.
    let mut winsize = unsafe { std::mem::zeroed::<libc::winsize>() };
    // SAFETY: ioctl writes to winsize when fd references a terminal.
    if unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &mut winsize) } == -1 {
        return Err(io::Error::last_os_error().into());
    }

    let width = if winsize.ws_col == 0 {
        80
    } else {
        winsize.ws_col
    };
    let height = if winsize.ws_row == 0 {
        24
    } else {
        winsize.ws_row
    };
    Ok(Size { width, height })
}

fn fit_width(text: &str, width: u16) -> String {
    let width = usize::from(width);
    if width == 0 {
        return String::new();
    }

    let mut fitted = text.chars().take(width).collect::<String>();
    let current = fitted.chars().count();
    if current < width {
        fitted.push_str(&" ".repeat(width - current));
    }
    fitted
}
