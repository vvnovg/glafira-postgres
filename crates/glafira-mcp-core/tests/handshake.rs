//! Рукопожатие шима (спец. P4 §3.3): `GLAFIRA-MCP/1 <token>\n` → `OK\n` или `ERR bad_token <message>\n`,
//! таймаут 5 с, строка не длиннее 256 байт.

use glafira_mcp_core::endpoint::{self, Endpoint, Listener};
use glafira_mcp_core::handshake::{self, HandshakeError, MAX_LINE, PREFIX, PROTOCOL, tokens_equal};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[test]
fn prefix_is_the_protocol_and_a_space() {
    assert_eq!(PROTOCOL, "GLAFIRA-MCP/1");
    assert_eq!(PREFIX, format!("{PROTOCOL} "));
}

#[tokio::test]
async fn accepts_right_token_and_keeps_following_bytes() {
    let (mut a, mut b) = tokio::io::duplex(1024);
    let srv = tokio::spawn(async move {
        handshake::server(&mut b, "t0k3n").await.unwrap();
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
    let srv = tokio::spawn(async move { handshake::server(&mut b, "right").await });
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
        handshake::server(&mut b, "t").await,
        Err(HandshakeError::TooLong)
    ));
}

#[tokio::test(start_paused = true)]
async fn silent_client_times_out() {
    let (_a, mut b) = tokio::io::duplex(64);
    assert!(matches!(
        handshake::server(&mut b, "t").await,
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
        handshake::server(&mut s, "t0k3n").await.unwrap();
        let mut buf = [0u8; 4];
        s.read_exact(&mut buf).await.unwrap();
        buf
    });
    let mut c = endpoint::connect(&ep).await.unwrap();
    handshake::client(&mut c, "t0k3n").await.unwrap();
    c.write_all(b"ping").await.unwrap();
    assert_eq!(&srv.await.unwrap(), b"ping");
}
