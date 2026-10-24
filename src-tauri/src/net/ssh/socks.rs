//! Соединение с узлом через SOCKS5-порт туннеля (`ssh -D`): имя узла разрешает бастион.

use std::net::SocketAddr;

use tokio::net::TcpStream;
use tokio_socks::tcp::Socks5Stream;

use crate::conn::pg::tcp_connect;
use crate::error::{AppError, AppResult, ErrorCode};

pub type SocksStream = Socks5Stream<TcpStream>;

/// TCP к `proxy` (с keepalive, как прямое соединение), затем SOCKS5 CONNECT к `host:port` по имени. `ssh -D`
/// отвечает успехом SOCKS5 ещё до того, как бастион откроет канал к узлу, поэтому недоступный узел виден не
/// ошибкой `connect`, а концом потока (EOF) при первом чтении, и разбирает его вызывающий. Своего срока у
/// `connect` нет: вызывающий оборачивает его в таймаут подключения.
pub async fn connect(proxy: SocketAddr, host: &str, port: u16) -> AppResult<SocksStream> {
    let tcp = tcp_connect(&proxy.ip().to_string(), proxy.port())
        .await
        .map_err(|e| socks_error(host, port, &e.to_string()))?;
    Socks5Stream::connect_with_socket(tcp, (host, port))
        .await
        .map_err(|e| socks_error(host, port, &e.to_string()))
}

/// `SSH_ERROR` вида `forwardFailed`. На практике срабатывает, только когда порт туннеля закрыт (процесс `ssh`
/// уже умер) или рукопожатие SOCKS5 оборвалось: недоступный узел сюда не попадает (см. `connect`).
fn socks_error(host: &str, port: u16, cause: &str) -> AppError {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Поддельный SOCKS5-прокси без аутентификации: CONNECT к имени → TCP к `upstream`, затем прокачка байтов.
    async fn fake_socks(upstream: std::net::SocketAddr) -> std::net::SocketAddr {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (mut s, _) = l.accept().await.unwrap();
                tokio::spawn(async move {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    let mut buf = [0u8; 262];
                    s.read_exact(&mut buf[..2]).await.unwrap();
                    let n = buf[1] as usize;
                    s.read_exact(&mut buf[..n]).await.unwrap();
                    s.write_all(&[5, 0]).await.unwrap();
                    s.read_exact(&mut buf[..4]).await.unwrap();
                    assert_eq!(
                        (buf[0], buf[1], buf[3]),
                        (5, 1, 3),
                        "CONNECT к доменному имени"
                    );
                    s.read_exact(&mut buf[..1]).await.unwrap();
                    let n = buf[0] as usize;
                    s.read_exact(&mut buf[..n]).await.unwrap();
                    assert_eq!(&buf[..n], b"pg-single");
                    s.read_exact(&mut buf[..2]).await.unwrap();
                    assert_eq!(u16::from_be_bytes([buf[0], buf[1]]), 5432);
                    let mut up = tokio::net::TcpStream::connect(upstream).await.unwrap();
                    s.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]).await.unwrap();
                    let _ = tokio::io::copy_bidirectional(&mut s, &mut up).await;
                });
            }
        });
        addr
    }

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
        let proxy = fake_socks(upstream).await;
        let mut stream = connect(proxy, "pg-single", 5432).await.unwrap();
        tokio::io::AsyncWriteExt::write_all(&mut stream, b"hello")
            .await
            .unwrap();
        let mut b = [0u8; 5];
        tokio::io::AsyncReadExt::read_exact(&mut stream, &mut b)
            .await
            .unwrap();
        assert_eq!(&b, b"hello");
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
