//! Соединение с узлом через SOCKS5-порт туннеля (`ssh -D`): имя узла разрешает бастион.

use std::net::SocketAddr;

use tokio::net::TcpStream;
use tokio_socks::tcp::Socks5Stream;

use crate::conn::pg::tcp_connect;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::net::ssh::process::{SshErrorKind, ssh_error};

pub type SocksStream = Socks5Stream<TcpStream>;

/// TCP к `proxy` (с keepalive, как прямое соединение), затем SOCKS5 CONNECT к `host:port` по имени. Недоступный с
/// бастиона узел виден по-разному в зависимости от клиента OpenSSH: старый отвечает успехом SOCKS5 ещё до открытия
/// канала, и узел виден концом потока (EOF) при первом чтении — его разбирает вызывающий; OpenSSH 10.x отвечает
/// только после открытия канала и при отказе закрывает соединение без ответа — это ошибка `connect`
/// (`socks_error`). Вид ошибки (`forwardFailed`) в обоих случаях один. Своего срока у `connect` нет: вызывающий
/// оборачивает его в таймаут подключения.
pub async fn connect(proxy: SocketAddr, host: &str, port: u16) -> AppResult<SocksStream> {
    let tcp = tcp_connect(&proxy.ip().to_string(), proxy.port())
        .await
        .map_err(|e| socks_error(host, port, &e.to_string()))?;
    Socks5Stream::connect_with_socket(tcp, (host, port))
        .await
        .map_err(|e| socks_error(host, port, &e.to_string()))
}

/// `SSH_ERROR` вида `forwardFailed`: порт туннеля закрыт (процесс `ssh` уже умер) или рукопожатие SOCKS5 оборвалось —
/// у OpenSSH 10.x так выглядит и узел, до которого бастион не достучался (см. `connect`, `forward_failed`).
pub(crate) fn socks_error(host: &str, port: u16, cause: &str) -> AppError {
    let mut e = app_err!(
        ErrorCode::SshError,
        "errors.ssh.socks",
        "The bastion could not connect to {{host}}:{{port}}",
        host = host,
        port = port
    )
    .with_details(cause);
    e.params.insert("kind".into(), "forwardFailed".into());
    e.retryable = true;
    e
}

/// Узел недоступен с бастиона (`forwardFailed`): `ssh -D` ответил успехом SOCKS5 и закрыл поток — канал к узлу не
/// открылся. `target` — цель ssh туннеля; адрес узла и причина — в `details`.
pub fn forward_failed(target: &str, host: &str, port: u16, cause: &str) -> AppError {
    ssh_error(
        SshErrorKind::ForwardFailed,
        target,
        &format!("{host}:{port}: {cause}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conn::testutil::socks_proxy_recording;

    #[tokio::test]
    async fn connect_goes_through_the_proxy_with_remote_name_resolution() {
        let echo = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream = echo.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut s, _) = echo.accept().await.unwrap();
            let mut b = [0u8; 5];
            tokio::io::AsyncReadExt::read_exact(&mut s, &mut b)
                .await
                .unwrap();
            tokio::io::AsyncWriteExt::write_all(&mut s, &b)
                .await
                .unwrap();
        });
        let (proxy, requests) = socks_proxy_recording(("127.0.0.1", upstream.port())).await;
        let mut stream = connect(proxy, "pg-single", 5432).await.unwrap();
        tokio::io::AsyncWriteExt::write_all(&mut stream, b"hello")
            .await
            .unwrap();
        let mut b = [0u8; 5];
        tokio::io::AsyncReadExt::read_exact(&mut stream, &mut b)
            .await
            .unwrap();
        assert_eq!(&b, b"hello");
        // CONNECT к доменному имени: адрес разрешает прокси (бастион), а не клиент.
        assert_eq!(
            *requests.lock().unwrap(),
            [(3u8, "pg-single".to_string(), 5432u16)]
        );
    }

    #[test]
    fn forward_failure_names_the_bastion_and_keeps_the_node_in_details() {
        let e = forward_failed("bastion", "pg-single", 5432, "early eof");
        assert_eq!(
            (e.code, e.key.as_str(), e.params["kind"].as_str()),
            (
                ErrorCode::SshError,
                "errors.ssh.forwardFailed",
                "forwardFailed"
            )
        );
        assert_eq!(e.params["target"], "bastion");
        assert_eq!(e.details.as_deref(), Some("pg-single:5432: early eof"));
        assert!(e.retryable);
    }

    #[tokio::test]
    async fn refused_proxy_is_an_ssh_error() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        drop(l);
        let e = connect(addr, "pg-single", 5432).await.unwrap_err();
        assert_eq!(
            (e.code, e.key.as_str()),
            (ErrorCode::SshError, "errors.ssh.socks")
        );
    }
}
