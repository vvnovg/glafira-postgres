//! Рукопожатие шима (спец. P4 §3.3): `GLAFIRA-MCP/1 <token>\n` → `OK\n` или `ERR bad_token <message>\n`,
//! таймаут 5 с, строка не длиннее 256 байт. P4c §3.3: билет фонового запуска `run=<билет>` после токена → id запуска
//! или `ERR bad_ticket <message>\n`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use glafira_mcp_core::endpoint::{self, Endpoint, Listener};
use glafira_mcp_core::handshake::{
    self, BAD_TICKET_MESSAGE, HandshakeError, MAX_LINE, PREFIX, PROTOCOL, RUN_PREFIX, TICKET_LEN,
    tokens_equal, valid_ticket,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const RUN: &str = "a1b2c3d";

/// Обычное рукопожатие: фоновых запусков нет.
fn no_runs(_: &str) -> Option<String> {
    None
}

fn ticket() -> String {
    "0123456789abcdef".repeat(4)
}

/// Проверка билета приложения: `ticket()` даёт запуск `RUN`, пока `live`.
fn run_check(live: Arc<AtomicBool>) -> impl Fn(&str) -> Option<String> + Send + Sync + 'static {
    move |t: &str| (live.load(Ordering::SeqCst) && t == ticket()).then(|| RUN.to_string())
}

/// Сервер с проверкой `check`, клиент с билетом `t`: ошибка клиента и итог сервера.
async fn refused(
    t: &str,
    check: impl Fn(&str) -> Option<String> + Send + Sync + 'static,
) -> (HandshakeError, Result<Option<String>, HandshakeError>) {
    let (mut a, mut b) = tokio::io::duplex(1024);
    let srv = tokio::spawn(async move { handshake::server(&mut b, "t0k3n", &check).await });
    let e = handshake::client_with_run(&mut a, "t0k3n", Some(t))
        .await
        .unwrap_err();
    (e, srv.await.unwrap())
}

#[test]
fn prefix_is_the_protocol_and_a_space() {
    assert_eq!(PROTOCOL, "GLAFIRA-MCP/1");
    assert_eq!(PREFIX, format!("{PROTOCOL} "));
}

#[tokio::test]
async fn accepts_right_token_and_keeps_following_bytes() {
    let (mut a, mut b) = tokio::io::duplex(1024);
    let srv = tokio::spawn(async move {
        handshake::server(&mut b, "t0k3n", &no_runs).await.unwrap();
        let mut rest = String::new();
        b.read_to_string(&mut rest).await.unwrap();
        rest
    });
    handshake::client(&mut a, "t0k3n").await.unwrap();
    a.write_all(b"{\"jsonrpc\":\"2.0\"}\n").await.unwrap();
    drop(a);
    assert_eq!(srv.await.unwrap(), "{\"jsonrpc\":\"2.0\"}\n");
}

#[tokio::test]
async fn rejects_wrong_token() {
    let (mut a, mut b) = tokio::io::duplex(1024);
    let srv = tokio::spawn(async move { handshake::server(&mut b, "right", &no_runs).await });
    let e = handshake::client(&mut a, "wrong").await.unwrap_err();
    assert!(
        matches!(&e, HandshakeError::Rejected { code, .. } if code == "bad_token"),
        "{e:?}"
    );
    assert!(e.to_string().contains("Invalid MCP token"));
    assert!(matches!(
        srv.await.unwrap(),
        Err(HandshakeError::Rejected { .. })
    ));
}

#[tokio::test]
async fn too_long_line_is_refused() {
    let (mut a, mut b) = tokio::io::duplex(4096);
    a.write_all(&[b'x'; MAX_LINE + 10]).await.unwrap();
    assert!(matches!(
        handshake::server(&mut b, "t", &no_runs).await,
        Err(HandshakeError::TooLong)
    ));
}

#[tokio::test(start_paused = true)]
async fn silent_client_times_out() {
    let (_a, mut b) = tokio::io::duplex(64);
    assert!(matches!(
        handshake::server(&mut b, "t", &no_runs).await,
        Err(HandshakeError::Timeout)
    ));
}

#[tokio::test]
async fn client_sees_a_server_that_closes_without_an_answer() {
    let (mut a, b) = tokio::io::duplex(1024);
    drop(b);
    let r = handshake::client(&mut a, "t").await;
    assert!(
        matches!(
            r,
            Err(HandshakeError::Io(_)) | Err(HandshakeError::Malformed)
        ),
        "{r:?}"
    );
}

#[test]
fn token_comparison() {
    assert!(tokens_equal("abc", "abc"));
    assert!(!tokens_equal("abc", "abd"));
    assert!(!tokens_equal("abc", "abcd"));
}

#[tokio::test]
async fn handshake_over_the_real_endpoint() {
    let dir = tempfile::tempdir().unwrap();
    let ep = Endpoint::for_app(dir.path(), "glafira-pg-test");
    let mut l = Listener::bind(&ep).await.unwrap();
    let srv = tokio::spawn(async move {
        let mut s = l.accept().await.unwrap();
        handshake::server(&mut s, "t0k3n", &no_runs).await.unwrap();
        let mut buf = [0u8; 4];
        s.read_exact(&mut buf).await.unwrap();
        buf
    });
    let mut c = endpoint::connect(&ep).await.unwrap();
    handshake::client(&mut c, "t0k3n").await.unwrap();
    c.write_all(b"ping").await.unwrap();
    assert_eq!(&srv.await.unwrap(), b"ping");
}

#[test]
fn ticket_is_64_hex_digits() {
    assert_eq!(RUN_PREFIX, "run=");
    assert_eq!(TICKET_LEN, 64);
    let t = ticket();
    assert!(valid_ticket(&t));
    let long = format!("{t}0");
    let spaced = format!("{} ", &t[..63]);
    for bad in [
        "",
        "xyz",
        &t[..63],
        long.as_str(),
        spaced.as_str(),
        "g".repeat(64).as_str(),
    ] {
        assert!(!valid_ticket(bad), "{bad:?}");
    }
}

#[tokio::test]
async fn ticket_in_the_handshake_gives_the_run_and_keeps_following_bytes() {
    let (mut a, mut b) = tokio::io::duplex(1024);
    let srv = tokio::spawn(async move {
        let check = run_check(Arc::new(AtomicBool::new(true)));
        let run = handshake::server(&mut b, "t0k3n", &check).await.unwrap();
        let mut rest = String::new();
        b.read_to_string(&mut rest).await.unwrap();
        (run, rest)
    });
    handshake::client_with_run(&mut a, "t0k3n", Some(&ticket()))
        .await
        .unwrap();
    a.write_all(b"{\"jsonrpc\":\"2.0\"}\n").await.unwrap();
    drop(a);
    assert_eq!(
        srv.await.unwrap(),
        (Some(RUN.to_string()), "{\"jsonrpc\":\"2.0\"}\n".to_string())
    );
}

#[tokio::test]
async fn old_handshake_has_no_run_and_asks_nothing() {
    let asked = Arc::new(AtomicUsize::new(0));
    let n = asked.clone();
    let (mut a, mut b) = tokio::io::duplex(1024);
    let srv = tokio::spawn(async move {
        let check = move |_: &str| {
            n.fetch_add(1, Ordering::SeqCst);
            Some(RUN.to_string())
        };
        handshake::server(&mut b, "t0k3n", &check).await
    });
    handshake::client(&mut a, "t0k3n").await.unwrap();
    assert_eq!(
        srv.await.unwrap().unwrap(),
        None,
        "старая форма — обычная сессия"
    );
    assert_eq!(asked.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn unknown_expired_or_malformed_ticket_is_bad_ticket() {
    let live = Arc::new(AtomicBool::new(true));
    let unknown = "f".repeat(64);
    let mut cases = vec![
        refused(&unknown, run_check(live.clone())).await,
        refused("xyz", run_check(live.clone())).await,
    ];
    live.store(false, Ordering::SeqCst);
    cases.push(refused(&ticket(), run_check(live.clone())).await);
    for (client, server) in cases {
        assert!(
            matches!(&client, HandshakeError::Rejected { code, message }
                if code == "bad_ticket" && message == BAD_TICKET_MESSAGE),
            "{client:?}"
        );
        assert_eq!(
            client.to_string(),
            "The background run is over: start it again in Glafira"
        );
        assert!(
            matches!(&server, Err(HandshakeError::Rejected { code, .. }) if code == "bad_ticket"),
            "{server:?}"
        );
    }
}

#[tokio::test]
async fn wrong_token_is_bad_token_and_the_ticket_is_not_checked() {
    let asked = Arc::new(AtomicUsize::new(0));
    let n = asked.clone();
    let (mut a, mut b) = tokio::io::duplex(1024);
    let srv = tokio::spawn(async move {
        let check = move |_: &str| {
            n.fetch_add(1, Ordering::SeqCst);
            Some(RUN.to_string())
        };
        handshake::server(&mut b, "right", &check).await
    });
    let e = handshake::client_with_run(&mut a, "wrong", Some(&ticket()))
        .await
        .unwrap_err();
    assert!(
        matches!(&e, HandshakeError::Rejected { code, .. } if code == "bad_token"),
        "{e:?}"
    );
    assert!(srv.await.unwrap().is_err());
    assert_eq!(
        asked.load(Ordering::SeqCst),
        0,
        "без верного токена билет не проверяется"
    );
}

#[tokio::test]
async fn a_line_with_a_ticket_fits_and_a_longer_one_is_refused() {
    let line = format!("{PREFIX}{} {RUN_PREFIX}{}", "f".repeat(64), ticket());
    assert!(line.len() < MAX_LINE, "{}", line.len());
    let (mut a, mut b) = tokio::io::duplex(4096);
    a.write_all(format!("{PREFIX}tok {RUN_PREFIX}{}\n", "a".repeat(MAX_LINE)).as_bytes())
        .await
        .unwrap();
    assert!(matches!(
        handshake::server(&mut b, "tok", &no_runs).await,
        Err(HandshakeError::TooLong)
    ));
}
