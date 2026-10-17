//! Hosts for tests: a [`HostConnector`] that reaches a host over a stream the
//! test controls instead of a command, and a host whose answers the test
//! scripts.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use embers_core::{MuxError, RequestId, Result};
use embers_protocol::{
    ClientMessage, FrameType, HelloResponse, PROTOCOL_VERSION, RawFrame, SOFTWARE_VERSION,
    ServerEnvelope, ServerEvent, ServerResponse, decode_client_message, encode_server_envelope,
    read_frame, write_frame,
};
use tokio::io::{DuplexStream, ReadHalf, WriteHalf};
use tokio::net::UnixStream;
use tokio::task::AbortHandle;

use crate::remote::{BoxFuture, HostConnector};
use crate::upstream::Upstream;

/// Reaches each host's server on its socket, through a relay that
/// [`Self::cut`] severs, as when the network goes down.
#[derive(Clone, Default)]
pub(crate) struct SocketHosts {
    inner: Arc<Mutex<SocketHostsInner>>,
}

#[derive(Default)]
struct SocketHostsInner {
    sockets: HashMap<String, PathBuf>,
    /// Hosts that cannot be reached now.
    down: Vec<String>,
    /// Hosts that never answer.
    silent: Vec<String>,
    /// How many connections each host was asked for.
    attempts: HashMap<String, usize>,
    relays: HashMap<String, Vec<AbortHandle>>,
}

impl SocketHosts {
    /// Reach `host` on `socket`.
    pub(crate) fn add(&self, host: &str, socket: PathBuf) {
        self.lock().sockets.insert(host.to_owned(), socket);
    }

    /// Sever the connections to `host` and refuse new ones until
    /// [`Self::restore`].
    pub(crate) fn cut(&self, host: &str) {
        let mut inner = self.lock();
        inner.down.push(host.to_owned());
        for relay in inner.relays.remove(host).unwrap_or_default() {
            relay.abort();
        }
    }

    /// Make `host` never answer, as when a connection hangs.
    pub(crate) fn silence(&self, host: &str) {
        self.lock().silent.push(host.to_owned());
    }

    /// How many connections `host` was asked for.
    pub(crate) fn attempts(&self, host: &str) -> usize {
        self.lock().attempts.get(host).copied().unwrap_or(0)
    }

    /// Let `host` be reached again.
    pub(crate) fn restore(&self, host: &str) {
        self.lock().down.retain(|down| down != host);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, SocketHostsInner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl HostConnector for SocketHosts {
    fn connect<'a>(&'a self, host: &'a str) -> BoxFuture<'a, Result<Upstream>> {
        Box::pin(async move {
            let (silent, socket) = {
                let mut inner = self.lock();
                *inner.attempts.entry(host.to_owned()).or_default() += 1;
                let silent = inner.silent.iter().any(|silent| silent == host);
                if inner.down.iter().any(|down| down == host) {
                    return Err(MuxError::transport(format!("{host} cannot be reached")));
                }
                (silent, inner.sockets.get(host).cloned())
            };
            if silent {
                return std::future::pending().await;
            }
            let socket = socket
                .ok_or_else(|| MuxError::transport(format!("no server is known for {host}")))?;
            let mut server = UnixStream::connect(&socket)
                .await
                .map_err(|error| MuxError::transport(format!("{host}: {error}")))?;
            let (mut relayed, ours) = tokio::io::duplex(64 * 1024);
            let relay = tokio::spawn(async move {
                let _ = tokio::io::copy_bidirectional(&mut relayed, &mut server).await;
            });
            self.lock()
                .relays
                .entry(host.to_owned())
                .or_default()
                .push(relay.abort_handle());
            let (reader, writer) = tokio::io::split(ours);
            Upstream::over(format!("{host}'s socket"), reader, writer).await
        })
    }
}

/// The far end of a host connection, played by a test: it reads what the
/// connection sends and answers only what, and when, the test says.
pub(crate) struct ScriptedHost {
    reader: ReadHalf<DuplexStream>,
    writer: WriteHalf<DuplexStream>,
}

impl ScriptedHost {
    /// A connection to a scripted host, its version check answered.
    pub(crate) async fn connect(host: &str) -> (Upstream, Self) {
        let (ours, theirs) = tokio::io::duplex(64 * 1024);
        let (reader, writer) = tokio::io::split(ours);
        let (their_reader, their_writer) = tokio::io::split(theirs);
        let mut scripted = Self {
            reader: their_reader,
            writer: their_writer,
        };
        let (upstream, ()) = tokio::join!(Upstream::over(host, reader, writer), async {
            let Some(ClientMessage::Hello(hello)) = scripted.next_request().await else {
                panic!("a connection starts with its version check");
            };
            scripted
                .respond(ServerResponse::Hello(HelloResponse {
                    request_id: hello.request_id,
                    protocol_version: PROTOCOL_VERSION,
                    software_version: SOFTWARE_VERSION.to_owned(),
                }))
                .await;
        });
        (upstream.expect("version check"), scripted)
    }

    /// The next request the connection sends, or `None` once it closed.
    pub(crate) async fn next_request(&mut self) -> Option<ClientMessage> {
        let frame = read_frame(&mut self.reader).await.expect("read a frame")?;
        Some(decode_client_message(&frame.payload).expect("decode a request"))
    }

    /// The next request, unless none comes within `wait`.
    pub(crate) async fn request_within(&mut self, wait: Duration) -> Option<ClientMessage> {
        tokio::time::timeout(wait, self.next_request())
            .await
            .ok()
            .flatten()
    }

    pub(crate) async fn respond(&mut self, response: ServerResponse) {
        let request_id = response.request_id().unwrap_or(RequestId(0));
        self.write(
            FrameType::Response,
            request_id,
            ServerEnvelope::Response(response),
        )
        .await;
    }

    pub(crate) async fn send_event(&mut self, event: ServerEvent) {
        self.write(FrameType::Event, RequestId(0), ServerEnvelope::Event(event))
            .await;
    }

    async fn write(
        &mut self,
        frame_type: FrameType,
        request_id: RequestId,
        envelope: ServerEnvelope,
    ) {
        let payload = encode_server_envelope(&envelope).expect("encode");
        write_frame(
            &mut self.writer,
            &RawFrame::new(frame_type, request_id, payload),
        )
        .await
        .expect("write a frame");
    }
}
