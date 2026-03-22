use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use embers_client::{
    ConfigManager, ConfiguredClient, KeyEvent, MouseButton, MouseEvent, MouseEventKind,
    MouseModifiers, MuxClient, RenderGrid, SocketTransport,
};
use embers_core::{CursorShape, MuxError, Result, SessionId, Size};
use embers_protocol::{BufferRequest, ClientMessage, ServerResponse, SessionRequest};
use tokio::sync::mpsc;
use unicode_width::UnicodeWidthStr;

const DEFAULT_SESSION_NAME: &str = "main";
const KEY_SEQUENCE_TIMEOUT: Duration = Duration::from_millis(15);
const KEY_SEQUENCE_CONTINUATION_TIMEOUT: Duration = Duration::from_millis(2);
const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(20);
const BRACKETED_PASTE_END: &[u8] = b"\x1b[201~";
const TERMINAL_ENTER_BASE_SEQUENCE: &str =
    "\x1b[?1049h\x1b[?1004h\x1b[?2004h\x1b[?25l\x1b[2J\x1b[H";
const TERMINAL_ENABLE_MOUSE_SEQUENCE: &str = "\x1b[?1002h\x1b[?1006h";
const TERMINAL_DISABLE_MOUSE_SEQUENCE: &str = "\x1b[?1006l\x1b[?1002l";

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

    let mut terminal = TerminalGuard::enter(mouse_capture_enabled(&configured))?;
    let (input_tx, mut input_rx) = mpsc::unbounded_channel();
    let _input_thread = spawn_input_thread(input_tx)?;

    let mut terminal_size = terminal.size()?;
    let mut dirty = true;
    loop {
        if dirty {
            terminal.sync_mouse_capture(mouse_capture_enabled(&configured))?;
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
            terminal.write_bytes(&drain_terminal_output(&mut configured))?;
            let status = configured.status_line(session_id, &socket_path);
            terminal.render(&grid, terminal_size, Some(&status))?;
            dirty = false;
        }

        loop {
            match input_rx.try_recv() {
                Ok(TerminalEvent::Key(KeyEvent::Ctrl('q'))) => return Ok(()),
                Ok(TerminalEvent::Key(key)) => {
                    let viewport = content_viewport(terminal_size);
                    configured.handle_key(session_id, viewport, key).await?;
                    terminal.write_bytes(&drain_terminal_output(&mut configured))?;
                    dirty = true;
                }
                Ok(TerminalEvent::Paste(bytes)) => {
                    let viewport = content_viewport(terminal_size);
                    configured.handle_paste(session_id, viewport, bytes).await?;
                    terminal.write_bytes(&drain_terminal_output(&mut configured))?;
                    dirty = true;
                }
                Ok(TerminalEvent::Focus(focused)) => {
                    let viewport = content_viewport(terminal_size);
                    configured
                        .handle_focus_event(session_id, viewport, focused)
                        .await?;
                    terminal.write_bytes(&drain_terminal_output(&mut configured))?;
                    dirty = true;
                }
                Ok(TerminalEvent::Mouse(mouse)) => {
                    let viewport = content_viewport(terminal_size);
                    configured.handle_mouse(session_id, viewport, mouse).await?;
                    terminal.write_bytes(&drain_terminal_output(&mut configured))?;
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
                terminal.write_bytes(&drain_terminal_output(&mut configured))?;
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
    let tabs = root.tabs.as_ref();
    Ok(tabs.is_none_or(|tabs| !tabs.tabs.is_empty()))
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

fn drain_terminal_output(configured: &mut ConfiguredClient<SocketTransport>) -> Vec<u8> {
    configured
        .drain_terminal_output()
        .into_iter()
        .flatten()
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum TerminalEvent {
    Key(KeyEvent),
    Mouse(MouseEvent),
    Paste(Vec<u8>),
    Focus(bool),
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
                match read_terminal_event(fd) {
                    Ok(Some(event)) => {
                        if tx.send(event).is_err() {
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

fn read_terminal_event(fd: libc::c_int) -> Result<Option<TerminalEvent>> {
    let Some(first) = read_byte(fd)? else {
        return Ok(None);
    };
    let event = match first {
        b'\r' | b'\n' => TerminalEvent::Key(KeyEvent::Enter),
        b'\t' => TerminalEvent::Key(KeyEvent::Tab),
        0x7f | 0x08 => TerminalEvent::Key(KeyEvent::Backspace),
        0x1b => read_escape_event(fd)?,
        0x01..=0x1a => TerminalEvent::Key(KeyEvent::Ctrl(char::from(b'a' + first - 1))),
        0x20..=0x7e => TerminalEvent::Key(KeyEvent::Char(char::from(first))),
        other => TerminalEvent::Key(decode_utf8_key(fd, other)?),
    };
    Ok(Some(event))
}

fn read_escape_event(fd: libc::c_int) -> Result<TerminalEvent> {
    let Some(next) = read_optional_byte(fd, KEY_SEQUENCE_TIMEOUT)? else {
        return Ok(TerminalEvent::Key(KeyEvent::Escape));
    };

    match next {
        b'[' => read_csi_event(fd),
        b'O' => read_ss3_event(fd),
        byte if byte.is_ascii() => Ok(TerminalEvent::Key(KeyEvent::Alt(char::from(byte)))),
        other => {
            let mut bytes = vec![0x1b, other];
            while let Some(extra) = read_optional_byte(fd, KEY_SEQUENCE_CONTINUATION_TIMEOUT)? {
                bytes.push(extra);
            }
            Ok(TerminalEvent::Key(KeyEvent::Bytes(bytes)))
        }
    }
}

fn read_csi_event(fd: libc::c_int) -> Result<TerminalEvent> {
    let bytes = read_control_sequence(fd, b'[')?;
    if bytes == b"\x1b[200~" {
        return Ok(TerminalEvent::Paste(read_bracketed_paste(fd)?));
    }
    Ok(parse_csi_event(&bytes).unwrap_or(TerminalEvent::Key(KeyEvent::Bytes(bytes))))
}

fn read_ss3_event(fd: libc::c_int) -> Result<TerminalEvent> {
    let mut bytes = vec![0x1b, b'O'];
    let Some(final_byte) = read_optional_byte(fd, KEY_SEQUENCE_CONTINUATION_TIMEOUT)? else {
        return Ok(TerminalEvent::Key(KeyEvent::Bytes(bytes)));
    };
    bytes.push(final_byte);
    let key = match final_byte {
        b'A' => Some(KeyEvent::Up),
        b'B' => Some(KeyEvent::Down),
        b'C' => Some(KeyEvent::Right),
        b'D' => Some(KeyEvent::Left),
        _ => None,
    };
    Ok(match key {
        Some(key) => TerminalEvent::Key(key),
        None => TerminalEvent::Key(KeyEvent::Bytes(bytes)),
    })
}

fn read_control_sequence(fd: libc::c_int, introducer: u8) -> Result<Vec<u8>> {
    let mut bytes = vec![0x1b, introducer];
    while let Some(next) = read_optional_byte(fd, KEY_SEQUENCE_CONTINUATION_TIMEOUT)? {
        bytes.push(next);
        if is_csi_final_byte(next) {
            break;
        }
    }
    Ok(bytes)
}

fn is_csi_final_byte(byte: u8) -> bool {
    (0x40..=0x7e).contains(&byte)
}

fn parse_csi_event(bytes: &[u8]) -> Option<TerminalEvent> {
    match bytes {
        b"\x1b[A" => Some(TerminalEvent::Key(KeyEvent::Up)),
        b"\x1b[B" => Some(TerminalEvent::Key(KeyEvent::Down)),
        b"\x1b[C" => Some(TerminalEvent::Key(KeyEvent::Right)),
        b"\x1b[D" => Some(TerminalEvent::Key(KeyEvent::Left)),
        b"\x1b[1~" | b"\x1b[H" => Some(TerminalEvent::Key(KeyEvent::Home)),
        b"\x1b[2~" => Some(TerminalEvent::Key(KeyEvent::Insert)),
        b"\x1b[3~" => Some(TerminalEvent::Key(KeyEvent::Delete)),
        b"\x1b[4~" | b"\x1b[F" => Some(TerminalEvent::Key(KeyEvent::End)),
        b"\x1b[5~" => Some(TerminalEvent::Key(KeyEvent::PageUp)),
        b"\x1b[6~" => Some(TerminalEvent::Key(KeyEvent::PageDown)),
        b"\x1b[I" => Some(TerminalEvent::Focus(true)),
        b"\x1b[O" => Some(TerminalEvent::Focus(false)),
        _ => parse_sgr_mouse(bytes).map(TerminalEvent::Mouse),
    }
}

fn parse_sgr_mouse(bytes: &[u8]) -> Option<MouseEvent> {
    let text = std::str::from_utf8(bytes).ok()?;
    let body = text.strip_prefix("\x1b[<")?;
    let (payload, suffix) = body.split_at(body.len().checked_sub(1)?);
    if suffix != "M" && suffix != "m" {
        return None;
    }

    let mut parts = payload.split(';');
    let code = parts.next()?.parse::<u16>().ok()?;
    let column = parts.next()?.parse::<u16>().ok()?.saturating_sub(1);
    let row = parts.next()?.parse::<u16>().ok()?.saturating_sub(1);
    if parts.next().is_some() {
        return None;
    }

    let modifiers = MouseModifiers {
        shift: (code & 0b00100) != 0,
        alt: (code & 0b01000) != 0,
        ctrl: (code & 0b10000) != 0,
    };
    let button_code = code & 0b11;
    let kind = if (code & 0b1_000000) != 0 {
        match button_code {
            0 => MouseEventKind::WheelUp,
            1 => MouseEventKind::WheelDown,
            _ => return None,
        }
    } else if (code & 0b100000) != 0 {
        MouseEventKind::Drag(mouse_button(button_code)?)
    } else if suffix == "m" {
        MouseEventKind::Release(mouse_button(button_code))
    } else {
        MouseEventKind::Press(mouse_button(button_code)?)
    };

    Some(MouseEvent {
        row,
        column,
        modifiers,
        kind,
    })
}

fn mouse_button(code: u16) -> Option<MouseButton> {
    match code {
        0 => Some(MouseButton::Left),
        1 => Some(MouseButton::Middle),
        2 => Some(MouseButton::Right),
        _ => None,
    }
}

fn read_bracketed_paste(fd: libc::c_int) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    loop {
        let Some(next) = read_byte(fd)? else {
            break;
        };
        bytes.push(next);
        if bytes.ends_with(BRACKETED_PASTE_END) {
            let new_len = bytes.len() - BRACKETED_PASTE_END.len();
            bytes.truncate(new_len);
            break;
        }
    }
    Ok(bytes)
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
    mouse_capture_enabled: bool,
}

impl TerminalGuard {
    fn enter(mouse_capture_enabled: bool) -> Result<Self> {
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
        match write!(stdout, "{}", terminal_enter_sequence(mouse_capture_enabled))
            .and_then(|()| stdout.flush())
        {
            Ok(()) => {}
            Err(error) => {
                let _ = set_terminal_mode(input_fd, &original_mode);
                return Err(error.into());
            }
        }

        Ok(Self {
            input_fd,
            original_mode,
            mouse_capture_enabled,
        })
    }

    fn size(&self) -> Result<Size> {
        terminal_size(io::stdout().as_raw_fd())
    }

    fn write_bytes(&self, bytes: &[u8]) -> Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        let mut stdout = io::stdout();
        stdout.write_all(bytes)?;
        stdout.flush()?;
        Ok(())
    }

    fn sync_mouse_capture(&mut self, enabled: bool) -> Result<()> {
        if self.mouse_capture_enabled == enabled {
            return Ok(());
        }
        let mut stdout = io::stdout();
        write!(
            stdout,
            "{}",
            if enabled {
                TERMINAL_ENABLE_MOUSE_SEQUENCE
            } else {
                TERMINAL_DISABLE_MOUSE_SEQUENCE
            }
        )?;
        stdout.flush()?;
        self.mouse_capture_enabled = enabled;
        Ok(())
    }

    fn render(&self, grid: &RenderGrid, terminal_size: Size, status: Option<&str>) -> Result<()> {
        let mut stdout = io::stdout();
        write!(stdout, "\x1b[H")?;
        for line in grid.ansi_lines() {
            write!(stdout, "{line}\x1b[K\r\n")?;
        }

        if terminal_size.height > grid.height() {
            let status = fit_width(status.unwrap_or_default(), terminal_size.width);
            write!(stdout, "\x1b[7m{status}\x1b[0m\x1b[K")?;
        }

        write!(stdout, "\x1b[J")?;
        if let Some(cursor) = grid.cursor() {
            write!(
                stdout,
                "\x1b[{} q\x1b[?25h\x1b[{};{}H",
                cursor_shape_code(cursor.shape),
                cursor.y.saturating_add(1),
                cursor.x.saturating_add(1)
            )?;
        } else {
            write!(stdout, "\x1b[?25l")?;
        }
        stdout.flush()?;
        Ok(())
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = set_terminal_mode(self.input_fd, &self.original_mode);
        let mut stdout = io::stdout();
        let _ = write!(
            stdout,
            "{}",
            terminal_exit_sequence(self.mouse_capture_enabled)
        );
        let _ = stdout.flush();
    }
}

fn mouse_capture_enabled(configured: &ConfiguredClient<SocketTransport>) -> bool {
    configured
        .config()
        .active_script()
        .loaded_config()
        .mouse
        .capture_enabled()
}

fn terminal_enter_sequence(mouse_capture_enabled: bool) -> String {
    let mut sequence = TERMINAL_ENTER_BASE_SEQUENCE.to_owned();
    if mouse_capture_enabled {
        sequence.push_str(TERMINAL_ENABLE_MOUSE_SEQUENCE);
    }
    sequence
}

fn terminal_exit_sequence(mouse_capture_enabled: bool) -> String {
    let mut sequence = String::from("\x1b[0m\x1b[2 q\x1b[?25h\x1b[?2004l");
    if mouse_capture_enabled {
        sequence.push_str(TERMINAL_DISABLE_MOUSE_SEQUENCE);
    }
    sequence.push_str("\x1b[?1004l\x1b[?1049l");
    sequence
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
    if width == 0 {
        return String::new();
    }

    let width = usize::from(width);
    let mut fitted = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let ch_width = UnicodeWidthStr::width(ch.encode_utf8(&mut [0; 4])).max(1);
        if used + ch_width > width {
            break;
        }
        fitted.push(ch);
        used += ch_width;
    }
    let current = UnicodeWidthStr::width(fitted.as_str());
    if current < width {
        fitted.push_str(&" ".repeat(width - current));
    }
    fitted
}

fn cursor_shape_code(shape: CursorShape) -> u8 {
    match shape {
        CursorShape::Block => 2,
        CursorShape::Underline => 4,
        CursorShape::Beam => 6,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        TERMINAL_DISABLE_MOUSE_SEQUENCE, TERMINAL_ENABLE_MOUSE_SEQUENCE, TerminalEvent,
        read_terminal_event, terminal_enter_sequence, terminal_exit_sequence,
    };
    use embers_client::{KeyEvent, MouseButton, MouseEvent, MouseEventKind, MouseModifiers};

    fn with_pipe<T>(bytes: &[u8], test: impl FnOnce(libc::c_int) -> T) -> T {
        let mut fds = [0; 2];
        // SAFETY: pipe writes two valid file descriptors into the provided array.
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        let read_fd = fds[0];
        let write_fd = fds[1];
        // SAFETY: write_fd is valid and bytes points to a readable buffer of the requested size.
        let written = unsafe { libc::write(write_fd, bytes.as_ptr().cast(), bytes.len()) };
        assert_eq!(written, bytes.len() as isize);
        // SAFETY: write_fd/read_fd are valid file descriptors from pipe.
        unsafe {
            libc::close(write_fd);
        }
        let result = test(read_fd);
        // SAFETY: read_fd is valid until explicitly closed here.
        unsafe {
            libc::close(read_fd);
        }
        result
    }

    #[test]
    fn parses_page_up_and_page_down_keys() {
        with_pipe(b"\x1b[1~\x1b[2~\x1b[3~\x1b[4~\x1b[5~\x1b[6~", |fd| {
            assert_eq!(
                read_terminal_event(fd).unwrap(),
                Some(TerminalEvent::Key(KeyEvent::Home))
            );
            assert_eq!(
                read_terminal_event(fd).unwrap(),
                Some(TerminalEvent::Key(KeyEvent::Insert))
            );
            assert_eq!(
                read_terminal_event(fd).unwrap(),
                Some(TerminalEvent::Key(KeyEvent::Delete))
            );
            assert_eq!(
                read_terminal_event(fd).unwrap(),
                Some(TerminalEvent::Key(KeyEvent::End))
            );
            assert_eq!(
                read_terminal_event(fd).unwrap(),
                Some(TerminalEvent::Key(KeyEvent::PageUp))
            );
            assert_eq!(
                read_terminal_event(fd).unwrap(),
                Some(TerminalEvent::Key(KeyEvent::PageDown))
            );
        });
    }

    #[test]
    fn parses_arrow_and_focus_events() {
        with_pipe(b"\x1b[A\x1b[I\x1b[O", |fd| {
            assert_eq!(
                read_terminal_event(fd).unwrap(),
                Some(TerminalEvent::Key(KeyEvent::Up))
            );
            assert_eq!(
                read_terminal_event(fd).unwrap(),
                Some(TerminalEvent::Focus(true))
            );
            assert_eq!(
                read_terminal_event(fd).unwrap(),
                Some(TerminalEvent::Focus(false))
            );
        });
    }

    #[test]
    fn parses_sgr_mouse_events() {
        with_pipe(
            b"\x1b[<0;12;7M\x1b[<64;3;5M\x1b[<32;10;4M\x1b[<3;10;4m",
            |fd| {
                assert_eq!(
                    read_terminal_event(fd).unwrap(),
                    Some(TerminalEvent::Mouse(MouseEvent {
                        row: 6,
                        column: 11,
                        modifiers: MouseModifiers::default(),
                        kind: MouseEventKind::Press(MouseButton::Left),
                    }))
                );
                assert_eq!(
                    read_terminal_event(fd).unwrap(),
                    Some(TerminalEvent::Mouse(MouseEvent {
                        row: 4,
                        column: 2,
                        modifiers: MouseModifiers::default(),
                        kind: MouseEventKind::WheelUp,
                    }))
                );
                assert_eq!(
                    read_terminal_event(fd).unwrap(),
                    Some(TerminalEvent::Mouse(MouseEvent {
                        row: 3,
                        column: 9,
                        modifiers: MouseModifiers::default(),
                        kind: MouseEventKind::Drag(MouseButton::Left),
                    }))
                );
                assert_eq!(
                    read_terminal_event(fd).unwrap(),
                    Some(TerminalEvent::Mouse(MouseEvent {
                        row: 3,
                        column: 9,
                        modifiers: MouseModifiers::default(),
                        kind: MouseEventKind::Release(None),
                    }))
                );
            },
        );
    }

    #[test]
    fn parses_bracketed_paste_payloads() {
        with_pipe(b"\x1b[200~hello\nworld\x1b[201~", |fd| {
            assert_eq!(
                read_terminal_event(fd).unwrap(),
                Some(TerminalEvent::Paste(b"hello\nworld".to_vec()))
            );
        });
    }

    #[test]
    fn terminal_guard_sequences_toggle_mouse_capture_with_config() {
        let with_mouse_enter = terminal_enter_sequence(true);
        let without_mouse_enter = terminal_enter_sequence(false);
        let with_mouse_exit = terminal_exit_sequence(true);
        let without_mouse_exit = terminal_exit_sequence(false);

        assert!(with_mouse_enter.contains("\x1b[?1004h"));
        assert!(with_mouse_enter.contains("\x1b[?2004h"));
        assert!(with_mouse_enter.contains(TERMINAL_ENABLE_MOUSE_SEQUENCE));
        assert!(with_mouse_exit.contains("\x1b[?1004l"));
        assert!(with_mouse_exit.contains("\x1b[?2004l"));
        assert!(with_mouse_exit.contains(TERMINAL_DISABLE_MOUSE_SEQUENCE));

        assert!(without_mouse_enter.contains("\x1b[?1004h"));
        assert!(without_mouse_enter.contains("\x1b[?2004h"));
        assert!(!without_mouse_enter.contains(TERMINAL_ENABLE_MOUSE_SEQUENCE));
        assert!(without_mouse_exit.contains("\x1b[?1004l"));
        assert!(without_mouse_exit.contains("\x1b[?2004l"));
        assert!(!without_mouse_exit.contains(TERMINAL_DISABLE_MOUSE_SEQUENCE));
    }
}
