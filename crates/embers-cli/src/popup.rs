//! `display-popup`: tmux's sizes and positions, and waiting until the popup
//! closes.
//!
//! Like tmux run from a shell, `display-popup` returns once the popup has
//! closed, with its command's exit status. To learn that status the popup's
//! buffer is created with `remain-on-exit on`, so the server keeps it after
//! the command exits instead of removing it with its popup; the waiting CLI
//! then closes the popup itself (or, with `-EE`, only after a successful
//! exit). If the CLI is interrupted it hands the popup back to the server's
//! usual handling first.

use std::path::Path;

use embers_core::{
    BufferId, FloatGeometry, FloatingId, MuxError, Result, SessionId, Size, new_request_id,
};
use embers_protocol::{
    BufferRecordState, BufferRequest, ClientMessage, FloatingRequest, ProtocolClient,
    ServerEnvelope, ServerResponse, SubscribeRequest,
};
use tokio::time::{Duration, sleep};

use crate::CliConnection;

/// The buffer option the server reads to keep an exited buffer in its view.
pub(crate) const REMAIN_ON_EXIT: &str = "remain-on-exit";

/// How often the popup is checked when no event arrives, in case one is
/// missed.
const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// A popup width or height: cells, or a percentage of the client's view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PopupLength {
    Cells(u16),
    Percent(u16),
}

/// A popup column or row: cells or a percentage of the view from the left or
/// top, `C` to centre it, or (for `-x`) `R` to put it at the right edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PopupPosition {
    Cells(u16),
    Percent(u16),
    Centre,
    End,
}

pub fn parse_popup_length(value: &str) -> std::result::Result<PopupLength, String> {
    match value.strip_suffix('%') {
        Some(percent) => percent
            .parse()
            .ok()
            .filter(|percent| (1..=100).contains(percent))
            .map(PopupLength::Percent)
            .ok_or_else(|| "expected a percentage from 1% to 100%".to_owned()),
        None => value
            .parse()
            .ok()
            .filter(|cells| *cells > 0)
            .map(PopupLength::Cells)
            .ok_or_else(|| "expected a number of cells or a percentage such as 90%".to_owned()),
    }
}

pub fn parse_popup_position(value: &str) -> std::result::Result<PopupPosition, String> {
    match value {
        "C" => Ok(PopupPosition::Centre),
        "R" => Ok(PopupPosition::End),
        _ => match value.strip_suffix('%') {
            Some(percent) => percent
                .parse()
                .ok()
                .filter(|percent| *percent <= 100)
                .map(PopupPosition::Percent)
                .ok_or_else(|| "expected a percentage from 0% to 100%".to_owned()),
            None => value.parse().map(PopupPosition::Cells).map_err(|_| {
                "expected a cell, a percentage such as 10%, C (centre) or R (right)".to_owned()
            }),
        },
    }
}

/// Where a popup goes in `viewport`. Like tmux, it defaults to half the view
/// each way, centred, and is kept inside the view.
pub(crate) fn popup_geometry(
    viewport: Size,
    width: Option<PopupLength>,
    height: Option<PopupLength>,
    x: Option<PopupPosition>,
    y: Option<PopupPosition>,
) -> FloatGeometry {
    let length = |value: PopupLength, extent: u16| -> u16 {
        let cells = match value {
            PopupLength::Cells(cells) => cells,
            PopupLength::Percent(percent) => {
                u16::try_from(u32::from(extent) * u32::from(percent) / 100).unwrap_or(extent)
            }
        };
        cells.clamp(1, extent.max(1))
    };
    let position = |value: PopupPosition, extent: u16, length: u16| -> u16 {
        let room = extent.saturating_sub(length);
        let start = match value {
            PopupPosition::Cells(cells) => cells,
            PopupPosition::Percent(percent) => {
                u16::try_from(u32::from(extent) * u32::from(percent) / 100).unwrap_or(extent)
            }
            PopupPosition::Centre => room / 2,
            PopupPosition::End => room,
        };
        start.min(room)
    };
    let width = length(width.unwrap_or(PopupLength::Percent(50)), viewport.width);
    let height = length(height.unwrap_or(PopupLength::Percent(50)), viewport.height);
    FloatGeometry::new(
        position(x.unwrap_or(PopupPosition::Centre), viewport.width, width),
        position(y.unwrap_or(PopupPosition::Centre), viewport.height, height),
        width,
        height,
    )
}

/// When a waited-for popup closes after its command exits: tmux `-E` (and
/// Embers without `-E`), or `-EE`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CloseOnExit {
    Always,
    OnSuccess,
}

/// A popup a `display-popup` is waiting for.
pub(crate) struct WaitedPopup {
    pub session_id: SessionId,
    pub floating_id: FloatingId,
    pub buffer_id: BufferId,
    pub close: CloseOnExit,
}

impl CliConnection {
    /// Wait until `popup` closes and return its command's exit status, or 0
    /// when it closed before the command exited or the status is unknown.
    ///
    /// The popup closes when its command exits (with `-EE`, exits with
    /// status 0), when someone closes it (`kill-popup`, a key binding), or
    /// with its session. A popup belongs to its session, not to a client, so
    /// it outlives a client detaching and the wait goes on. When the popup
    /// closes first, its command is ended like `kill-popup` ends it, unless
    /// the buffer was moved elsewhere.
    pub(crate) async fn wait_for_popup(
        &mut self,
        socket: &Path,
        popup: WaitedPopup,
    ) -> Result<i32> {
        let mut events = ProtocolClient::connect(socket)
            .await
            .map_err(|error| MuxError::transport(error.to_string()))?;
        match events
            .request(&ClientMessage::Subscribe(SubscribeRequest {
                request_id: new_request_id(),
                session_id: Some(popup.session_id),
            }))
            .await
            .map_err(|error| MuxError::transport(error.to_string()))?
        {
            ServerResponse::SubscriptionAck(_) => {}
            other => {
                return Err(MuxError::protocol(format!(
                    "unexpected response to display-popup subscribe: {other:?}"
                )));
            }
        }
        let mut exit_code = None;
        let interrupted = interrupt_signal();
        tokio::pin!(interrupted);
        loop {
            let open = self.popup_is_open(&popup).await.map_err(server_gone)?;
            let buffer = match self
                .request(ClientMessage::Buffer(BufferRequest::Get {
                    request_id: new_request_id(),
                    buffer_id: popup.buffer_id,
                }))
                .await
            {
                Ok(ServerResponse::Buffer(response)) => Some(response.buffer),
                Ok(_) => None,
                Err(error) if error.is_not_found() => None,
                Err(error) => return Err(server_gone(error)),
            };
            let exited = buffer
                .as_ref()
                .is_some_and(|buffer| buffer.state == BufferRecordState::Exited);
            if exited {
                exit_code = buffer.as_ref().and_then(|buffer| buffer.exit_code);
            }
            if !open {
                if !exited && buffer.is_some() {
                    self.kill_detached_buffers(vec![popup.buffer_id]).await?;
                }
                return Ok(exit_code.unwrap_or(0));
            }
            if exited && (popup.close == CloseOnExit::Always || exit_code == Some(0)) {
                self.close_popup(&popup).await?;
                return Ok(exit_code.unwrap_or(0));
            }
            tokio::select! {
                envelope = events.recv() => match envelope {
                    Ok(Some(ServerEnvelope::Event(_) | ServerEnvelope::Response(_))) => {}
                    Ok(None) | Err(_) => return Err(server_gone(MuxError::transport("connection closed"))),
                },
                () = sleep(POLL_INTERVAL) => {}
                signal = &mut interrupted => {
                    self.release_popup(&popup).await;
                    return Ok(128 + signal);
                }
            }
        }
    }

    async fn popup_is_open(&mut self, popup: &WaitedPopup) -> Result<bool> {
        match self.session_snapshot(popup.session_id).await {
            Ok(snapshot) => Ok(snapshot
                .floating
                .iter()
                .any(|floating| floating.id == popup.floating_id)),
            Err(error) if error.is_not_found() => Ok(false),
            Err(error) => Err(error),
        }
    }

    async fn close_popup(&mut self, popup: &WaitedPopup) -> Result<()> {
        match self
            .request(ClientMessage::Floating(FloatingRequest::Close {
                request_id: new_request_id(),
                floating_id: popup.floating_id,
            }))
            .await
        {
            Ok(_) => {}
            Err(error) if error.is_not_found() => {}
            Err(error) => return Err(error),
        }
        // The server removes the exited buffer with its view.
        Ok(())
    }

    /// Stop waiting: hand the popup back to the server, which closes it when
    /// its command exits (or has exited), as it does for a popup nobody
    /// waits for.
    async fn release_popup(&mut self, popup: &WaitedPopup) {
        let result = self
            .request(ClientMessage::Buffer(BufferRequest::SetUserOption {
                request_id: new_request_id(),
                buffer_id: popup.buffer_id,
                key: REMAIN_ON_EXIT.to_owned(),
                value: match popup.close {
                    CloseOnExit::Always => None,
                    CloseOnExit::OnSuccess => Some("failed".to_owned()),
                },
            }))
            .await;
        if let Err(error) = result {
            tracing::warn!(%error, "failed to release the popup display-popup waited for");
        }
    }
}

fn server_gone(error: MuxError) -> MuxError {
    if error.is_not_found() {
        return error;
    }
    MuxError::transport(format!(
        "the server went away before the popup closed: {error}"
    ))
}

/// Resolves with the signal number when the CLI is asked to stop.
#[cfg(unix)]
async fn interrupt_signal() -> i32 {
    use tokio::signal::unix::{SignalKind, signal};
    let (Ok(mut interrupt), Ok(mut terminate), Ok(mut hangup)) = (
        signal(SignalKind::interrupt()),
        signal(SignalKind::terminate()),
        signal(SignalKind::hangup()),
    ) else {
        return std::future::pending().await;
    };
    tokio::select! {
        _ = interrupt.recv() => libc::SIGINT,
        _ = terminate.recv() => libc::SIGTERM,
        _ = hangup.recv() => libc::SIGHUP,
    }
}

#[cfg(not(unix))]
async fn interrupt_signal() -> i32 {
    let _ = tokio::signal::ctrl_c().await;
    2
}

#[cfg(test)]
mod tests {
    use super::*;

    fn size(width: u16, height: u16) -> Size {
        Size { width, height }
    }

    #[test]
    fn popups_default_to_half_the_view_centred() {
        assert_eq!(
            popup_geometry(size(100, 40), None, None, None, None),
            FloatGeometry::new(25, 10, 50, 20)
        );
    }

    #[test]
    fn popup_sizes_and_positions_take_cells_percentages_and_tokens() {
        let percent = |value| Some(PopupLength::Percent(value));
        assert_eq!(
            popup_geometry(size(100, 40), percent(90), percent(90), None, None),
            FloatGeometry::new(5, 2, 90, 36)
        );
        assert_eq!(
            popup_geometry(
                size(100, 40),
                Some(PopupLength::Cells(20)),
                Some(PopupLength::Cells(5)),
                Some(PopupPosition::End),
                Some(PopupPosition::Percent(10)),
            ),
            FloatGeometry::new(80, 4, 20, 5)
        );
        // Kept inside the view.
        assert_eq!(
            popup_geometry(
                size(100, 40),
                Some(PopupLength::Cells(200)),
                Some(PopupLength::Cells(10)),
                Some(PopupPosition::Cells(90)),
                Some(PopupPosition::Cells(35)),
            ),
            FloatGeometry::new(0, 30, 100, 10)
        );
    }

    #[test]
    fn popup_arguments_parse_like_tmux() {
        assert_eq!(parse_popup_length("90%"), Ok(PopupLength::Percent(90)));
        assert_eq!(parse_popup_length("12"), Ok(PopupLength::Cells(12)));
        assert!(parse_popup_length("0").is_err());
        assert!(parse_popup_length("101%").is_err());
        assert_eq!(parse_popup_position("C"), Ok(PopupPosition::Centre));
        assert_eq!(parse_popup_position("R"), Ok(PopupPosition::End));
        assert_eq!(parse_popup_position("0"), Ok(PopupPosition::Cells(0)));
        assert_eq!(parse_popup_position("25%"), Ok(PopupPosition::Percent(25)));
        assert!(parse_popup_position("M").is_err());
    }
}
