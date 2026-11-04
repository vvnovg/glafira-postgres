//! Перенос из Glafira for Kafka (crates/glafira-mcp/src/handshake.rs): добавлена константа `PROTOCOL`, тесты —
//! в `tests/handshake.rs`.
//! Рукопожатие шима (спец. P4 §3.3): первая строка соединения — `GLAFIRA-MCP/1 <token>\n`, ответ — `OK\n` или
//! `ERR <code> <message>\n`. Дальше по соединению идёт JSON-RPC MCP.

use std::fmt;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const PROTOCOL: &str = "GLAFIRA-MCP/1";
/// Начало строки рукопожатия: протокол и пробел.
pub const PREFIX: &str = "GLAFIRA-MCP/1 ";
/// Длиннее строка рукопожатия быть не может: префикс и 64 hex-символа токена, с запасом.
pub const MAX_LINE: usize = 256;
pub const TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug)]
pub enum HandshakeError {
    Io(std::io::Error),
    Timeout,
    TooLong,
    /// Соединение закрыто до конца строки или строка не по протоколу.
    Malformed,
    Rejected {
        code: String,
        message: String,
    },
}

impl fmt::Display for HandshakeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "MCP connection failed: {e}"),
            Self::Timeout => write!(f, "Glafira did not answer the MCP handshake in time"),
            Self::TooLong | Self::Malformed => write!(f, "Unexpected MCP handshake"),
            Self::Rejected { message, .. } => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for HandshakeError {}

/// Читает строку до `\n` побайтно, чтобы не забрать из потока первые байты MCP.
pub async fn read_line<R: AsyncRead + Unpin>(r: &mut R) -> Result<String, HandshakeError> {
    let mut buf = Vec::new();
    loop {
        let mut b = [0u8; 1];
        if r.read(&mut b).await.map_err(HandshakeError::Io)? == 0 {
            return Err(HandshakeError::Malformed);
        }
        if b[0] == b'\n' {
            break;
        }
        buf.push(b[0]);
        if buf.len() > MAX_LINE {
            return Err(HandshakeError::TooLong);
        }
    }
    String::from_utf8(buf).map_err(|_| HandshakeError::Malformed)
}

/// Сравнение за время, не зависящее от места первого расхождения.
pub fn tokens_equal(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

async fn write_line<W: AsyncWrite + Unpin>(w: &mut W, line: &str) -> Result<(), HandshakeError> {
    w.write_all(line.as_bytes())
        .await
        .map_err(HandshakeError::Io)?;
    w.flush().await.map_err(HandshakeError::Io)
}

/// Сторона шима: отправляет токен и ждёт `OK`.
pub async fn client<S: AsyncRead + AsyncWrite + Unpin>(
    s: &mut S,
    token: &str,
) -> Result<(), HandshakeError> {
    write_line(s, &format!("{PREFIX}{token}\n")).await?;
    let line = tokio::time::timeout(TIMEOUT, read_line(s))
        .await
        .map_err(|_| HandshakeError::Timeout)??;
    if line == "OK" {
        return Ok(());
    }
    match line.strip_prefix("ERR ") {
        Some(rest) => {
            let (code, message) = rest.split_once(' ').unwrap_or((rest, ""));
            Err(HandshakeError::Rejected {
                code: code.to_string(),
                message: message.to_string(),
            })
        }
        None => Err(HandshakeError::Malformed),
    }
}

/// Сторона приложения: принимает соединение только с верным токеном.
pub async fn server<S: AsyncRead + AsyncWrite + Unpin>(
    s: &mut S,
    token: &str,
) -> Result<(), HandshakeError> {
    let line = tokio::time::timeout(TIMEOUT, read_line(s))
        .await
        .map_err(|_| HandshakeError::Timeout)??;
    match line.strip_prefix(PREFIX) {
        Some(t) if tokens_equal(t, token) => write_line(s, "OK\n").await,
        _ => {
            let message = "Invalid MCP token: restart the MCP server in your client";
            write_line(s, &format!("ERR bad_token {message}\n")).await?;
            Err(HandshakeError::Rejected {
                code: "bad_token".into(),
                message: message.into(),
            })
        }
    }
}
