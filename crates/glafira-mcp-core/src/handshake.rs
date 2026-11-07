//! Перенос из Glafira for Kafka (crates/glafira-mcp/src/handshake.rs): добавлена константа `PROTOCOL`, тесты —
//! в `tests/handshake.rs`.
//! Рукопожатие шима (спец. P4 §3.3, P4c §3.3). Первая строка соединения — `GLAFIRA-MCP/1 <token>\n`, у шима фонового
//! запуска — `GLAFIRA-MCP/1 <token> run=<билет>\n`. Ответ — `OK\n` или `ERR <code> <message>\n`. Дальше по
//! соединению идёт JSON-RPC MCP.

use std::fmt;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const PROTOCOL: &str = "GLAFIRA-MCP/1";
/// Начало строки рукопожатия: протокол и пробел.
pub const PREFIX: &str = "GLAFIRA-MCP/1 ";
/// P4c: необязательная часть строки после токена и пробела — билет фонового запуска.
pub const RUN_PREFIX: &str = "run=";
/// Билет — 32 случайных байта в hex.
pub const TICKET_LEN: usize = 64;
/// Длиннее строка рукопожатия быть не может: префикс, 64 hex-символа токена и билет (147 байт), с запасом.
pub const MAX_LINE: usize = 256;
pub const TIMEOUT: Duration = Duration::from_secs(5);
const BAD_TOKEN_MESSAGE: &str = "Invalid MCP token: restart the MCP server in your client";
/// Отказ `bad_ticket`: билет неизвестен, запуск закончился или после токена не билет.
pub const BAD_TICKET_MESSAGE: &str = "The background run is over: start it again in Glafira";

/// Проверка билета приложением (`Gate::run_for_ticket`): id запуска или `None`.
pub type TicketCheck = dyn Fn(&str) -> Option<String> + Sync;

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

/// Билет — ровно `TICKET_LEN` hex-символов.
pub fn valid_ticket(s: &str) -> bool {
    s.len() == TICKET_LEN && s.bytes().all(|b| b.is_ascii_hexdigit())
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
    client_with_run(s, token, None).await
}

/// Сторона шима фонового запуска (P4c): токен и билет `run_ticket`; без билета — как [`client`].
pub async fn client_with_run<S: AsyncRead + AsyncWrite + Unpin>(
    s: &mut S,
    token: &str,
    run_ticket: Option<&str>,
) -> Result<(), HandshakeError> {
    let line = match run_ticket {
        Some(t) => format!("{PREFIX}{token} {RUN_PREFIX}{t}\n"),
        None => format!("{PREFIX}{token}\n"),
    };
    write_line(s, &line).await?;
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

/// Отказ: `ERR <code> <message>` клиенту и ошибка серверу.
async fn reject<S: AsyncWrite + Unpin>(
    s: &mut S,
    code: &str,
    message: &str,
) -> Result<Option<String>, HandshakeError> {
    write_line(s, &format!("ERR {code} {message}\n")).await?;
    Err(HandshakeError::Rejected {
        code: code.into(),
        message: message.into(),
    })
}

/// Сторона приложения: принимает соединение только с верным токеном. Билет фонового запуска (`run=<билет>` после
/// токена) проверяет `run_for_ticket` — и только после верного токена. Итог — id запуска. Неизвестный или истёкший
/// билет и любая другая часть после токена — `ERR bad_ticket`.
pub async fn server<S: AsyncRead + AsyncWrite + Unpin>(
    s: &mut S,
    token: &str,
    run_for_ticket: &TicketCheck,
) -> Result<Option<String>, HandshakeError> {
    let line = tokio::time::timeout(TIMEOUT, read_line(s))
        .await
        .map_err(|_| HandshakeError::Timeout)??;
    let Some(rest) = line.strip_prefix(PREFIX) else {
        return reject(s, "bad_token", BAD_TOKEN_MESSAGE).await;
    };
    let (t, tail) = match rest.split_once(' ') {
        Some((t, tail)) => (t, Some(tail)),
        None => (rest, None),
    };
    if !tokens_equal(t, token) {
        return reject(s, "bad_token", BAD_TOKEN_MESSAGE).await;
    }
    let run = match tail {
        None => None,
        Some(tail) => match tail
            .strip_prefix(RUN_PREFIX)
            .filter(|x| valid_ticket(x))
            .and_then(run_for_ticket)
        {
            Some(id) => Some(id),
            None => return reject(s, "bad_ticket", BAD_TICKET_MESSAGE).await,
        },
    };
    write_line(s, "OK\n").await?;
    Ok(run)
}
