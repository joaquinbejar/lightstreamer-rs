//! End-to-end regression for the HTTP long-polling establishment budget, over a
//! real socket and the public API.
//!
//! A polling `bind_session` legitimately blocks up to `LS_idle_millis` while the
//! server waits for a notification — that is what long polling *is*
//! [`docs/spec/02-session-lifecycle.md` §7.1]. The generic stream-establishment
//! timeout must therefore exceed the idle time, or every quiet poll cycle is
//! killed before the server can answer and the client connects and then delivers
//! nothing.
//!
//! This test reproduces exactly that on a loopback server: it withholds every
//! stream response for **1.5 s**, longer than the **1 s** open timeout the
//! client is configured with. Before the fix the client aborted each poll at
//! 1 s and looped forever with "not established within 1s"; after it, the
//! establishment budget is the open timeout plus the idle time, so the client
//! waits the response out, binds, and delivers a real update.
//!
//! Unlike a scripted in-process transport, this exercises the *actual*
//! `tokio::time::timeout` that wraps the transport's `open_stream` — a real TCP
//! read blocking on a real socket — which is where the live failure lived.

// A test harness: panicking on an unexpected shape is the point, and the loopback
// server slices its own small buffers.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures_util::StreamExt as _;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};

use lightstreamer_rs::{
    Client, ClientConfig, ConnectionOptions, FieldSchema, ItemGroup, ServerAddress, Subscription,
    SubscriptionEvent, SubscriptionMode, Transport,
};

/// How long the server withholds every stream response — longer than the
/// client's open timeout, shorter than open timeout + idle.
const HOLD: Duration = Duration::from_millis(1500);

/// The open timeout the client is configured with. A polling establishment
/// budget of just this would abort the withheld poll; the fix makes it this plus
/// the idle time.
const OPEN_TIMEOUT: Duration = Duration::from_secs(1);

/// Reads one HTTP request — head plus `Content-Length` body — from `socket`.
async fn read_request(socket: &mut TcpStream) -> String {
    let mut buffer = Vec::new();
    let mut scratch = [0u8; 4096];
    loop {
        if let Some(head_end) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&buffer[..head_end]);
            let content_length = head
                .lines()
                .find_map(|line| {
                    let lower = line.to_ascii_lowercase();
                    lower
                        .strip_prefix("content-length:")
                        .and_then(|value| value.trim().parse::<usize>().ok())
                })
                .unwrap_or(0);
            if buffer.len() >= head_end + 4 + content_length {
                return String::from_utf8_lossy(&buffer).into_owned();
            }
        }
        match socket.read(&mut scratch).await {
            Ok(0) | Err(_) => return String::from_utf8_lossy(&buffer).into_owned(),
            Ok(read) => buffer.extend_from_slice(&scratch[..read]),
        }
    }
}

/// The head of a streaming poll response, whose body runs until the socket
/// closes (no `Content-Length`), so the server can write it incrementally.
const STREAM_HEAD: &[u8] = b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n";

/// The bound window between `CONOK` and `LOOP` in which a real polling server
/// holds the connection open waiting for a notification. The client uses it to
/// send its subscribe control request, exactly as against a live server.
const BOUND_WINDOW: Duration = Duration::from_millis(500);

/// Writes a fixed-length `200 OK` response — used for the immediate control
/// reply.
async fn write_fixed(socket: &mut TcpStream, body: &str) {
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = socket.write_all(response.as_bytes()).await;
    let _ = socket.flush().await;
}

/// Extracts the `LS_reqId` value from a request body, defaulting to `1`.
fn request_id(request: &str) -> String {
    request
        .rsplit("LS_reqId=")
        .next()
        .map(|tail| {
            tail.chars()
                .take_while(char::is_ascii_alphanumeric)
                .collect::<String>()
        })
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| "1".to_owned())
}

/// Serves one connection: a create/bind poll (withheld by `HOLD`) or a control
/// request (answered at once). Data is delivered on a bind only once a
/// subscription control request has been seen.
async fn serve_connection(mut socket: TcpStream, subscribed: Arc<AtomicBool>) {
    let request = read_request(&mut socket).await;
    let path = request.lines().next().unwrap_or_default();

    if path.contains("create_session") {
        // The whole response is withheld past the open timeout: the client must
        // wait it out, which is the establishment-budget invariant under test.
        tokio::time::sleep(HOLD).await;
        let _ = socket.write_all(STREAM_HEAD).await;
        let _ = socket
            .write_all(b"CONOK,S1,50000,15000,*\r\nLOOP,0\r\n")
            .await;
        let _ = socket.flush().await;
    } else if path.contains("bind_session") {
        // `CONOK` is likewise withheld past the open timeout…
        tokio::time::sleep(HOLD).await;
        let _ = socket.write_all(STREAM_HEAD).await;
        let _ = socket.write_all(b"CONOK,S1,50000,15000,*\r\n").await;
        let _ = socket.flush().await;
        // …then the connection is held open, as a real poll is, giving the
        // client a bound window to issue its subscribe control request.
        tokio::time::sleep(BOUND_WINDOW).await;
        if subscribed.load(Ordering::SeqCst) {
            let _ = socket.write_all(b"SUBOK,1,1,2\r\nU,1,1,ACME|1.5\r\n").await;
        }
        let _ = socket.write_all(b"LOOP,0\r\n").await;
        let _ = socket.flush().await;
    } else if path.contains("control") {
        // A subscribe (or any control request): from now on, binds carry data.
        subscribed.store(true, Ordering::SeqCst);
        write_fixed(&mut socket, &format!("REQOK,{}\r\n", request_id(&request))).await;
    } else {
        write_fixed(&mut socket, "").await;
    }
}

/// Accepts connections and handles each concurrently, so a withheld poll never
/// blocks the parallel control connection.
async fn serve(listener: TcpListener, subscribed: Arc<AtomicBool>) {
    loop {
        match listener.accept().await {
            Ok((socket, _)) => {
                let subscribed = Arc::clone(&subscribed);
                tokio::spawn(serve_connection(socket, subscribed));
            }
            Err(_) => return,
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn http_polling_waits_out_a_withheld_poll_and_delivers_data() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let address: SocketAddr = listener.local_addr().expect("local addr");
    let subscribed = Arc::new(AtomicBool::new(false));
    let server = tokio::spawn(serve(listener, Arc::clone(&subscribed)));

    let config = ClientConfig::builder(
        ServerAddress::try_new(format!("http://{address}")).expect("valid address"),
    )
    .with_transport(Transport::HttpPolling)
    .with_options(ConnectionOptions::default().with_open_timeout(OPEN_TIMEOUT))
    .build()
    .expect("valid config");

    // The create poll is withheld 1.5 s, past the 1 s open timeout. Before the
    // fix this never bound and `connect` never returned Ok.
    let (client, _events) = tokio::time::timeout(Duration::from_secs(20), Client::connect(config))
        .await
        .expect("connect must not hang waiting out the poll")
        .expect("connect must succeed despite the withheld poll response");

    let mut updates = client
        .subscribe(Subscription::new(
            SubscriptionMode::Merge,
            ItemGroup::from_items(["item1"]).expect("valid item"),
            FieldSchema::from_fields(["stock_name", "last_price"]).expect("valid schema"),
        ))
        .await
        .expect("subscribe queued");

    // A real update must arrive — the client is not merely connecting and then
    // going silent, which is the exact defect this guards.
    let update = tokio::time::timeout(Duration::from_secs(25), async {
        while let Some(event) = updates.next().await {
            if let SubscriptionEvent::Update(update) = event {
                return Some(update);
            }
        }
        None
    })
    .await
    .expect("an update must arrive within the budget")
    .expect("the subscription stream must yield an update");

    assert_eq!(
        update
            .field_by_name("last_price")
            .and_then(|value| value.text()),
        Some("1.5"),
        "the withheld poll cycle delivered its data"
    );

    drop(client);
    server.abort();
}
