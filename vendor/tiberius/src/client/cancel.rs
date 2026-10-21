//! tulka patch (TULKA_PATCHES.md, "Cancellation"): out-of-band cancellation
//! of the request in flight on a connection.
//!
//! A [`CancelHandle`] never touches the socket itself. It raises a flag shared
//! with the connection and wakes the task that is polling the connection (the
//! one driving the in-flight `QueryStream`, `query` future, etc.). That task
//! writes the Attention packet on its next poll, from inside
//! `Connection::poll_next`, where it already has exclusive access to the
//! transport. This keeps the single-owner design of the connection intact: no
//! split socket, no lock, no unsafe aliasing.

use futures_util::task::AtomicWaker;
use std::fmt;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};

/// The state shared between a connection and its cancel handles.
#[derive(Debug, Default)]
pub(crate) struct CancelSignal {
    /// Set by [`CancelHandle::cancel`], taken by the connection when it sends
    /// the Attention packet, and cleared when a new request is sent.
    requested: AtomicBool,
    /// The waker of the task currently polling the connection's read side.
    waker: AtomicWaker,
}

/// A cloneable handle that cancels the request in flight on a [`Client`],
/// even while a result stream borrows the client.
///
/// Obtained with [`Client::cancel_handle`]. It can be moved to another task
/// (for example a UI "Stop" button) and used concurrently with the stream.
///
/// # Example
///
/// ```no_run
/// # use tiberius::{Client, Config};
/// # use tokio_util::compat::TokioAsyncWriteCompatExt;
/// # use futures_util::stream::TryStreamExt;
/// # #[tokio::main]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// # let config = Config::new();
/// # let tcp = tokio::net::TcpStream::connect(config.get_addr()).await?;
/// # let mut client = Client::connect(config, tcp.compat_write()).await?;
/// let handle = client.cancel_handle();
///
/// tokio::spawn(async move {
///     tokio::time::sleep(std::time::Duration::from_millis(500)).await;
///     handle.cancel().await
/// });
///
/// let result: tiberius::Result<Vec<_>> = client
///     .simple_query("WAITFOR DELAY '00:00:30'")
///     .await?
///     .try_collect()
///     .await;
///
/// assert!(matches!(result, Err(tiberius::error::Error::Cancelled)));
/// // The client is in sync and can run the next request.
/// client.simple_query("SELECT 1").await?.into_row().await?;
/// # Ok(())
/// # }
/// ```
///
/// [`Client`]: crate::Client
/// [`Client::cancel_handle`]: crate::Client::cancel_handle
#[derive(Clone)]
pub struct CancelHandle {
    signal: Weak<CancelSignal>,
}

impl fmt::Debug for CancelHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CancelHandle")
            .field("connected", &(self.signal.strong_count() > 0))
            .finish()
    }
}

impl CancelHandle {
    /// Requests cancellation of the request currently in flight on the
    /// client.
    ///
    /// The future resolves as soon as the request is signalled; it does not
    /// wait for the server. The task that polls the in-flight result stream
    /// (or `query`/`execute` future) then sends a TDS Attention packet and
    /// keeps reading, across message boundaries, until the server's `DONE`
    /// token with the `DONE_ATTN` bit. At that point the stream ends with
    /// [`Error::Cancelled`] (items received before that are still yielded) and
    /// the client can be used again.
    ///
    /// If the result stream is dropped after cancelling (whether or not the
    /// Attention was already sent), the next request on the client sends the
    /// Attention if needed and first discards the rest of the cancelled
    /// response up to the acknowledgement. It waits only for the server to
    /// stop and for the data already in flight, not for the abandoned request
    /// to finish. When the dropped stream stopped in the middle of a token
    /// (the usual case for rows), the acknowledgement is located at the packet
    /// level.
    ///
    /// Calling this while no request is in flight, or after the whole response
    /// has already arrived, is a no-op: sending a new request clears any
    /// pending cancellation. So is a cancel issued before the request has been
    /// written to the wire: it is discarded when the request is sent.
    ///
    /// # Errors
    ///
    /// Fails with an [`Error::Io`] of kind `NotConnected` when the client was
    /// dropped.
    ///
    /// [`Error::Cancelled`]: crate::error::Error::Cancelled
    /// [`Error::Io`]: crate::error::Error::Io
    pub fn cancel(&self) -> impl Future<Output = crate::Result<()>> + Send + 'static {
        let result = match self.signal.upgrade() {
            Some(signal) => {
                signal.requested.store(true, Ordering::Release);
                signal.waker.wake();
                Ok(())
            }
            None => Err(crate::Error::Io {
                kind: std::io::ErrorKind::NotConnected,
                message: "the client of this cancel handle was dropped".into(),
            }),
        };

        std::future::ready(result)
    }
}

/// Per-connection cancellation bookkeeping, owned by `Connection`.
#[derive(Debug, Default)]
pub(crate) struct AttentionState {
    signal: Arc<CancelSignal>,
    /// An Attention packet was queued on the transport by `poll_next` but the
    /// transport has not finished flushing it yet.
    pub(crate) flush_pending: bool,
    /// An Attention was sent and the acknowledging `DONE_ATTN` has not been
    /// read yet. While set, the token stream reads past end-of-message
    /// boundaries instead of ending.
    pub(crate) awaiting_ack: bool,
    /// Part of a token has been consumed from the read buffer but the token is
    /// not fully decoded (a result stream was dropped mid-token). The token
    /// stream cannot resume from such a position.
    pub(crate) mid_token: bool,
    /// The last bytes consumed from the read buffer since the Attention was
    /// sent, for the packet-level search of the acknowledgement.
    pub(crate) consumed_tail: Vec<u8>,
}

impl AttentionState {
    pub(crate) fn handle(&self) -> CancelHandle {
        CancelHandle {
            signal: Arc::downgrade(&self.signal),
        }
    }

    /// Registers the waker of the task reading the connection, so a cancel
    /// from another task wakes it up.
    pub(crate) fn register(&self, waker: &std::task::Waker) {
        self.signal.waker.register(waker);
    }

    pub(crate) fn is_requested(&self) -> bool {
        self.signal.requested.load(Ordering::Acquire)
    }

    /// Takes a pending cancellation request, if any.
    pub(crate) fn take_request(&self) -> bool {
        self.signal.requested.swap(false, Ordering::AcqRel)
    }

    /// Drops a pending cancellation request: it targeted a request that has
    /// already finished.
    pub(crate) fn clear_request(&self) {
        self.signal.requested.store(false, Ordering::Release);
    }

    /// Appends consumed bytes to `consumed_tail`, keeping only the last
    /// `CONSUMED_TAIL_LEN` (at least the size of a DONE token).
    pub(crate) fn record_consumed(&mut self, bytes: &[u8]) {
        const CONSUMED_TAIL_LEN: usize = 16;

        let bytes = &bytes[bytes.len().saturating_sub(CONSUMED_TAIL_LEN)..];
        self.consumed_tail.extend_from_slice(bytes);
        let excess = self.consumed_tail.len().saturating_sub(CONSUMED_TAIL_LEN);
        self.consumed_tail.drain(..excess);
    }
}
