//! Соединение с узлом через SOCKS5-порт туннеля (`ssh -D`): имя узла разрешает бастион.

use std::net::SocketAddr;

use tokio::net::TcpStream;
use tokio_socks::tcp::Socks5Stream;

use crate::conn::pg::tcp_connect;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::net::ssh::process::{SshErrorKind, ssh_error};

pub type SocksStream = Socks5Stream<TcpStream>;

/// TCP к `proxy` (с keepalive, как прямое соединение), затем SOCKS5 CONNECT к `host:port` по имени; `target` — цель
/// ssh туннеля (для сообщений). Порт туннеля не принял TCP — туннель не работает (`tunnel_down`). Недоступный с
/// бастиона узел виден по-разному в зависимости от клиента OpenSSH: старый отвечает успехом SOCKS5 ещё до открытия
/// канала, и узел виден концом потока (EOF) при первом чтении — его разбирает вызывающий (`forward_failed`);
/// OpenSSH 10.x отвечает только после открытия канала и при отказе закрывает соединение без ответа — это ошибка
/// рукопожатия здесь. Ошибка в обоих случаях одна: `forward_failed` с узлом в `params`. Своего срока у `connect` нет:
/// вызывающий оборачивает его в таймаут подключения.
pub async fn connect(
    proxy: SocketAddr,
    target: &str,
    host: &str,
    port: u16,
) -> AppResult<SocksStream> {
    let tcp = tcp_connect(&proxy.ip().to_string(), proxy.port())
        .await
        .map_err(|e| tunnel_down(target, proxy.port(), &e.to_string()))?;
    Socks5Stream::connect_with_socket(tcp, (host, port))
        .await
        .map_err(|e| forward_failed(target, host, port, &e.to_string()))
}

/// Порт SOCKS туннеля не принимает соединения: процесс `ssh` только что умер (супервизор его перезапустит) или
/// туннель закрыт. Вид `exited`, повторяемая; `port` — локальный порт туннеля, причина — в `details`.
pub fn tunnel_down(target: &str, socks_port: u16, cause: &str) -> AppError {
    let mut e = app_err!(
        ErrorCode::SshError,
        "errors.ssh.tunnelDown",
        "The SSH tunnel to {{target}} is not running (local port {{port}} refused)",
        target = target,
        port = socks_port
    )
    .with_details(cause);
    e.params
        .insert("kind".into(), SshErrorKind::Exited.name().into());
    e.retryable = true;
    e
}

/// Узел недоступен с бастиона (`forwardFailed`): рукопожатие SOCKS5 оборвалось после соединения с портом туннеля
/// (OpenSSH 10.x) или `ssh -D` ответил успехом SOCKS5 и сразу закрыл поток (старые клиенты) — канал к узлу не
/// открылся. `target` — цель ssh туннеля; узел — в `params.host` и `params.port`, причина — в `details`.
pub fn forward_failed(target: &str, host: &str, port: u16, cause: &str) -> AppError {
    ssh_error(
        SshErrorKind::ForwardFailed {
            host: host.to_string(),
            port,
        },
        target,
        cause,
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
        let mut stream = connect(proxy, "bastion", "pg-single", 5432).await.unwrap();
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
    fn forward_failure_names_the_bastion_and_the_node() {
        let e = forward_failed("bastion", "pg-single", 5432, "early eof");
        assert_eq!(
            (e.code, e.key.as_str(), e.params["kind"].as_str()),
            (
                ErrorCode::SshError,
                "errors.ssh.forwardFailed",
                "forwardFailed"
            )
        );
        assert_eq!(
            (
                e.params["target"].as_str(),
                e.params["host"].as_str(),
                e.params["port"].as_str()
            ),
            ("bastion", "pg-single", "5432")
        );
        assert_eq!(
            e.message,
            "The bastion bastion could not reach pg-single:5432"
        );
        assert_eq!(e.details.as_deref(), Some("early eof"));
        assert!(e.retryable);
    }

    /// Порт туннеля закрыт (процесс ssh умер) — туннель не работает (`exited`, повторяемая), а не «бастион не
    /// достучался до узла».
    #[tokio::test]
    async fn refused_proxy_means_the_tunnel_is_down() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        drop(l);
        let e = connect(addr, "bastion", "pg-single", 5432)
            .await
            .unwrap_err();
        assert_eq!(
            (e.code, e.key.as_str(), e.params["kind"].as_str()),
            (ErrorCode::SshError, "errors.ssh.tunnelDown", "exited")
        );
        assert_eq!(e.params["target"], "bastion");
        assert_eq!(e.params["port"], addr.port().to_string());
        assert_eq!(
            e.message,
            format!(
                "The SSH tunnel to bastion is not running (local port {} refused)",
                addr.port()
            )
        );
        assert!(e.retryable && e.details.is_some());
    }

    /// OpenSSH 10.x: порт туннеля принял соединение, но на запрос CONNECT закрыл его без ответа (бастион не
    /// достучался до узла) — сбой проброса с узлом в `params`.
    #[tokio::test]
    async fn dropped_handshake_is_a_forward_failure() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut tcp, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 512];
            let _ = tcp.read(&mut buf).await;
            let _ = tcp.write_all(&[5, 0]).await;
            let _ = tcp.read(&mut buf).await;
        });
        let e = connect(proxy, "bastion", "nonexistent.invalid", 5432)
            .await
            .unwrap_err();
        assert_eq!(
            (e.key.as_str(), e.params["kind"].as_str()),
            ("errors.ssh.forwardFailed", "forwardFailed"),
            "{e:?}"
        );
        assert_eq!(
            (e.params["host"].as_str(), e.params["port"].as_str()),
            ("nonexistent.invalid", "5432")
        );
    }
}
