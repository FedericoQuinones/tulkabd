# Tulka patches to Tiberius

This directory is a vendored copy of **tiberius 0.13.0** exactly as published
on crates.io (`.cargo_vcs_info.json`: upstream commit
`7fd75bb7070fe6ea41374698abf78928bb983bda`), including its license files
(`LICENSE-MIT.txt`, `LICENSE-APACHE.txt`). Only the registry marker
`.cargo-ok` was left out. On top of it, tulka applies the minimal patches
listed below. Every changed spot in the code carries a `tulka patch` comment,
so `grep -rn "tulka patch"` lists them all.

tulka uses it as a path dependency (`crates/tulka-mssql/Cargo.toml`) with
`default-features = false, features = ["tds73", "rustls", "chrono"]`. The
crate is excluded from the tulka workspace (root `Cargo.toml`), so it builds
and tests as a standalone package with its own `Cargo.lock` and `target/`.

## Running the tests

```sh
# Patch tests (skipped, and passing, unless TULKA_MSSQL_TEST is set).
eval "$(bash scripts/mssql-test.sh)"   # from the repository root
cd vendor/tiberius
cargo test --no-default-features --features "tds73 rustls chrono" --test tulka_patches

# Upstream unit tests.
cargo test --no-default-features --features "tds73 rustls chrono" --lib
```

The upstream integration tests (`tests/query.rs`, `bulk.rs`, `command.rs`,
...) do not compile from the published crate, with or without these patches:
they need the workspace-only `runtimes-macro` crate, which upstream does not
publish. Run only the targets above.

## Updating or dropping the fork

To move to a newer upstream release, copy the new crates.io source over this
directory and re-apply the patches below (or drop the ones upstream now
covers). The integration tests in `tests/tulka_patches.rs` describe the
required behaviour and must keep passing. If upstream covers all three
features, delete `vendor/tiberius` and point `crates/tulka-mssql` back at the
crates.io release.

---

## 1. Informational messages: `QueryItem::Info`

**What.** `QueryStream` yields `QueryItem::Info(InfoMessage)` for every INFO
token (`PRINT`, `RAISERROR` with severity 10 or lower, `SET STATISTICS`
output, ...), in the order the server sent it:

```rust
pub struct InfoMessage {
    pub number: u32,  // 0 for PRINT, 50000 for an ad hoc RAISERROR
    pub state: u8,
    pub class: u8,    // the server sends severity 10 as class 0
    pub message: String,
    pub line: u32,
}
```

**Why.** Upstream decodes INFO tokens and only logs them as a `tracing` DEBUG
event (`ReceivedToken::Info` is dropped by `QueryStream::poll_next`'s
`_ => continue`). A SQL client must show `PRINT` output and server notices.

**Files.**
- `src/tds/stream/query.rs`: `QueryItem::Info`, `InfoMessage` (+ `From<TokenInfo>`),
  the `poll_next` arm, `forward_to_metadata` stops at it (otherwise `query()`
  and `simple_query()` would skip the messages sent before the first result
  set), the item helpers (`as_row`, `into_row_stream`, `collect_results`, ...)
  ignore it.
- `src/result.rs`: re-export `InfoMessage` from the crate root.
- `src/tds/stream/token.rs`, `src/tds/codec/token/token_info.rs`: comments
  only (the token is no longer dead code).

**Not covered.** `CommandStream` (`Command::exec`) and `ExecuteResult`
(`Client::execute`) still drop INFO tokens. `QueryStream::columns()` still
consumes (and so drops) the tokens it skips while peeking for metadata.

**Drop when** upstream surfaces INFO messages from `QueryStream`.

## 2. Per-statement completion: `QueryItem::Done`

**What.** `QueryStream` yields `QueryItem::Done(DoneInfo)` once per statement,
in order, interleaved with the metadata, rows and messages of that statement:

```rust
pub struct DoneInfo {
    pub rows_affected: Option<u64>,
}
```

There is one item per DONE / DONEPROC / DONEINPROC token the server sends, in
order, with `rows_affected: Some(n)` when the `DONE_COUNT` status bit is set
and `None` otherwise (DDL, `SET`, `PRINT`, `USE`, everything under
`SET NOCOUNT ON`). This is not a strict one-item-per-statement mapping: nested
procedures, triggers and dynamic SQL add their own items, `SET NOCOUNT ON`
suppresses some, and statements such as `DECLARE` emit none. Two DONE tokens
yield nothing:
- the DONE carrying `DONE_ATTN` (the acknowledgement of a cancel, patch 3);
- for RPC requests only (`Client::query` and `Query::query`, which run
  `sp_executesql`), the closing DONEPROC without `DONE_MORE`, which ends the
  RPC itself. In a plain batch (`Client::simple_query`) a DONEPROC ends an
  `EXEC` statement and is reported.

**Why.** Upstream drops DONE tokens in `QueryStream`; row counts are only
available from `Client::execute` (which discards rows) or from `Command`
(a named-procedure RPC, not a batch). A SQL client needs "N rows affected"
per statement of a batch that also returns result sets.

**Files.**
- `src/tds/stream/query.rs`: `QueryItem::Done`, `DoneInfo`, `surfaced()`,
  the `rpc` flag and `QueryStream::new_rpc`, the `poll_next` arm,
  `forward_to_metadata`.
- `src/client.rs` (`Client::query`), `src/query.rs` (`Query::query`): build the
  stream with `QueryStream::new_rpc`.
- `src/tds/codec/token/token_done.rs`: `TokenDone::row_count()` and
  `TokenDone::has_more()`.
- `src/result.rs`: re-export `DoneInfo` from the crate root.

**Compatibility.** `QueryItem` is not `#[non_exhaustive]`, so exhaustive
`match`es on it need a new arm (upstream's own `tests/query.rs` got
`_ => {}` arms for that reason, and the doc example in `query.rs` too).
`into_results`, `into_first_result`, `into_row` and `into_row_stream`
behave as before. Server errors are still reported as upstream does: the
first error of the response, at the end of the stream.

**Drop when** upstream surfaces per-statement DONE information from
`QueryStream`.

## 3. Cancellation: `CancelHandle`, `Error::Cancelled`, `cancel_query` fix

**What.**
- `Client::cancel_handle(&self) -> CancelHandle`. `CancelHandle` is
  `Clone + Send + Sync + 'static`; `CancelHandle::cancel(&self)` returns
  `impl Future<Output = tiberius::Result<()>> + Send + 'static` and works
  while a `QueryStream` (or a `query`/`simple_query`/`execute` future) borrows
  the client. It fails only with `Error::Io { kind: NotConnected, .. }` when
  the client was dropped.
- The in-flight stream sends a TDS Attention, keeps reading (across
  end-of-message packets) until the `DONE` with `DONE_ATTN`, and then ends
  with the new `Error::Cancelled`. Items received before that are still
  yielded (for example the `Done` of the aborted statement). The connection
  is in sync afterwards. `Client::execute` ends with `Error::Cancelled` the
  same way (tested); `Command::exec` reads through the same token stream and
  should too (not tested).
- If the stream is dropped after the cancel (whether or not the Attention was
  already sent), the next request on the client (or `cancel_query`) sends the
  Attention if needed and discards the rest of the cancelled response up to
  its acknowledgement before running. It waits only for the server to stop
  and for the data already in flight, not for the abandoned request to
  finish.
- A cancel issued before the request has been written to the wire is
  discarded when the request is sent.
- A cancel while no request is in flight, or after the whole response has
  already arrived, is a no-op: sending a request clears any pending cancel.
- The existing `Client::cancel_query()` is fixed (see below).

**Why.** Upstream has only `Client::cancel_query(&mut self)`, which cannot run
while a stream borrows the client (so a UI Stop button cannot use it), and is
broken against SQL Server 2022 (16.0.4305): the server answers an Attention
with two messages, the aborted request's `DONE` (status `Error`) and then a
separate message with `DONE_ATTN`. Upstream's `flush_done_attention` stops at
the first end-of-message, returns `Protocol("Never got a DONE token
acknowledging the Attention signal.")` and leaves the connection one response
behind (every later request reads the previous response). Dropping a stream
never sends an Attention, so the next request waits for the whole abandoned
response (for example a 30 s `WAITFOR`).

**Design.** `Connection` stays the single owner of the socket; the handle
never writes to it. The handle holds a `Weak` to a small shared signal (an
`AtomicBool` "cancel requested" plus an `AtomicWaker`). `Connection::poll_next`
(the packet read path that every token decode goes through) registers the
reading task's waker and, when a cancel is requested while a response is still
arriving (`!flushed`), queues the Attention packet on the transport
(`poll_ready` / `start_send` / `poll_flush`) before polling the next packet.
`cancel()` sets the flag and wakes the reading task, so a task parked on a
long `WAITFOR` wakes up, sends the Attention and goes back to reading. No
socket split, lock or `unsafe` is needed, and no request can be interleaved
with the Attention because only the connection's owner writes. Alternative
considered: sharing the write half behind a mutex. Rejected because it would
mean splitting `Framed<MaybeTlsStream<S>>` (and the TLS stream) into halves,
a much larger change, and the packet id counter in `Context` would need
sharing too. The cost of the chosen design: the Attention is sent only when
the stream (or future) is being polled, or by the next request. A consumer
that holds a stream without polling it should poll it or drop it after
cancelling.

After sending, `Connection` is "awaiting the ack". While it is, the token
stream (`TokenStream::try_unfold`) does not end at an end-of-message packet but
continues with the next message. When it decodes a DONE / DONEPROC / DONEINPROC
with `DONE_ATTN`, it clears the state and, at the end of that message, ends the
stream with `Error::Cancelled`. `QueryStream` does not yield the ack itself.

**Dropped streams.** The token stream also records whether it is in the
middle of a token (bytes consumed, token not complete). A dropped row stream
is usually in that state: it was suspended waiting for a packet, and SQL
Server fills packets without regard to token boundaries. When the
acknowledgement is still expected from that state (from `flush_stream` or
`cancel_query`), token parsing cannot resume, so the acknowledgement is located
at the packet level instead (`drain_attention_ack_by_packets`). Every server
message ends with a fixed-size DONE-family token (13 bytes from TDS 7.2), and
`DONE_ATTN` is always the final token of its message, so at each
end-of-message the last 13 bytes are checked for a DONE / DONEPROC /
DONEINPROC token (`0xFD`..`0xFF`) with the `DONE_ATTN` bit. Otherwise the
next message is read. While an Attention is unacknowledged, `poll_read` keeps
the last 16 consumed bytes, so a `DONE_ATTN` split across packets and partly
read by the dropped stream is still recognised. When the buffer is at a
token boundary (for example a stream dropped during a `WAITFOR`, or between
items), the token-level drain is used as before. The connection is poisoned
only if the transport fails or closes during the drain.

`flush_stream`'s plain packet drain now reads the transport directly (not
through `poll_next`), so a concurrent cancel cannot send an Attention in the
middle of a packet-level drain, whose acknowledgement would then be discarded
unseen.

**`Client::cancel_query` fix (same code path).** `cancel_request` writes the
Attention through the same helper, then drains with
`TokenStream::flush_done_attention`, which now reads across messages thanks to
the "awaiting ack" state. It returns `Ok(())` without contacting the server
when the response has fully arrived, and does not send a second Attention if
a `CancelHandle` already sent one. Its return type changed from
`Result<TokenDone>` to `Result<()>` (crate-private).

**Not changed.** The default `command_timeout` (30 s per round trip) is
untouched. A command timeout still poisons the connection; it is not a
cancel.

**Files.**
- `src/client/cancel.rs` (new): `CancelSignal`, `CancelHandle`,
  `AttentionState` (with `consumed_tail` / `record_consumed`).
- `src/client/connection.rs`: `attention` field (all constructors),
  `send` clears a stale cancel, `cancel_request`, `write_attention`,
  `drain_attention_ack`, `drain_attention_ack_by_packets` with the helpers
  `keep_last` and `is_done_attention`, `cancel_handle`,
  `awaiting_attention_ack`, `attention_acknowledged`,
  `continue_with_next_message`, `set_mid_token`, `poll_attention`,
  `flush_stream`, `poll_next`, `poll_read` (records consumed bytes while an
  Attention is unacknowledged).
- `src/tds/stream/token.rs`: `TokenStream::cancelled`, `try_unfold` reads past
  end-of-message while awaiting the ack, tracks mid-token, detects the ack and
  ends with `Error::Cancelled`; server-free unit tests at the end of the
  `tests` module.
- `src/error.rs`: `Error::Cancelled`.
- `src/client.rs`: `Client::cancel_handle`, `cancel_query` docs and return.
- `src/lib.rs`: re-export `CancelHandle`.

**Drop when** upstream offers a cancel that works while a stream is in flight
and reads past end-of-message up to `DONE_ATTN`. If upstream only fixes
`cancel_query`, the multi-message drain can be dropped but `CancelHandle` must
stay.

## 4. Build plumbing

- `Cargo.toml`: a `[[test]]` entry for `tests/tulka_patches.rs` (the published
  manifest sets `autotests = false`).
- `tests/tulka_patches.rs` (new): integration tests for patches 1 to 3. They
  need `TULKA_MSSQL_TEST` (`server=...;port=...;user=...;password=...`) and
  return early, passing, when it is unset.
- `tests/query.rs`: two `_ => {}` match arms for the new `QueryItem` variants.

## 5. Removed files

- `docker/` (upstream's SQL Server test containers and certificates) is
  deleted. It held test TLS private keys (`certs/customCA.key`,
  `certs/server.key`, `certs/server.pem`, `certs/passphrase.txt`) that secret
  scanners and GitHub push protection flag, and nothing in the build or in the
  tests used here needs it (upstream's `docker-compose.yml` and
  `tests/custom-cert.rs` reference it, but those are not run). Do not restore
  it when updating the vendored copy.
- The upstream unit tests that read those fixtures are marked
  `#[ignore = "tulka patch: needs the removed docker/certs fixtures ..."]`:
  10 in `src/client/tls_stream/certs.rs`, 9 in `rustls_tls_stream.rs`, and 3
  each in `native_tls_stream.rs` and `opentls_tls_stream.rs`. They exercise
  certificate and key file parsing only; `cargo test --lib` reports them as
  ignored (19 with `rustls`, 13 with the default `native-tls`).
