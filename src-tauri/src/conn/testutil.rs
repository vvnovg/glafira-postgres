//! Помощники тестов TLS без Docker: тестовые CA и сертификаты, «сервер PostgreSQL» на loopback.
//! Сервер отвечает на SSLRequest, делает TLS-рукопожатие и после стартового сообщения клиента
//! отдаёт `ErrorResponse` 28000 — по нему тест видит, что рукопожатие прошло.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use openssl::asn1::Asn1Time;
use openssl::bn::BigNum;
use openssl::ec::{EcGroup, EcKey};
use openssl::hash::MessageDigest;
use openssl::nid::Nid;
use openssl::pkey::{PKey, Private};
use openssl::ssl::{SslAcceptor, SslMethod};
use openssl::x509::extension::{BasicConstraints, KeyUsage, SubjectAlternativeName};
use openssl::x509::{X509, X509NameBuilder};

/// Сертификат вместе с закрытым ключом.
pub struct Issued {
    pub cert: X509,
    pub key: PKey<Private>,
}

fn new_key() -> PKey<Private> {
    let group = EcGroup::from_curve_name(Nid::X9_62_PRIME256V1).unwrap();
    PKey::from_ec_key(EcKey::generate(&group).unwrap()).unwrap()
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// `issuer == None` — самоподписанный. `ca` — сертификат удостоверяющего центра; `dns` — SAN.
fn issue(cn: &str, issuer: Option<&Issued>, ca: bool, dns: Option<&str>, serial: u32) -> Issued {
    // Начало — с запасом назад: часы тестовой машины могут чуть расходиться.
    let now = unix_now();
    issue_with_validity(cn, issuer, ca, dns, serial, now - 3600, now + 2 * 86_400)
}

/// Как `issue`, но со сроком действия `[not_before, not_after]` (секунды Unix).
fn issue_with_validity(
    cn: &str,
    issuer: Option<&Issued>,
    ca: bool,
    dns: Option<&str>,
    serial: u32,
    not_before: i64,
    not_after: i64,
) -> Issued {
    let key = new_key();
    let mut name = X509NameBuilder::new().unwrap();
    name.append_entry_by_text("CN", cn).unwrap();
    let name = name.build();
    let mut b = X509::builder().unwrap();
    b.set_version(2).unwrap();
    b.set_serial_number(&BigNum::from_u32(serial).unwrap().to_asn1_integer().unwrap())
        .unwrap();
    b.set_subject_name(&name).unwrap();
    match issuer {
        Some(i) => b.set_issuer_name(i.cert.subject_name()).unwrap(),
        None => b.set_issuer_name(&name).unwrap(),
    }
    b.set_pubkey(&key).unwrap();
    b.set_not_before(&Asn1Time::from_unix(not_before).unwrap())
        .unwrap();
    b.set_not_after(&Asn1Time::from_unix(not_after).unwrap())
        .unwrap();
    if ca {
        b.append_extension(BasicConstraints::new().critical().ca().build().unwrap())
            .unwrap();
        b.append_extension(
            KeyUsage::new()
                .critical()
                .key_cert_sign()
                .crl_sign()
                .build()
                .unwrap(),
        )
        .unwrap();
    }
    if let Some(dns) = dns {
        let ext = SubjectAlternativeName::new()
            .dns(dns)
            .build(&b.x509v3_context(issuer.map(|i| i.cert.as_ref()), None))
            .unwrap();
        b.append_extension(ext).unwrap();
    }
    b.sign(issuer.map_or(&key, |i| &i.key), MessageDigest::sha256())
        .unwrap();
    Issued {
        cert: b.build(),
        key,
    }
}

/// Самоподписанный сертификат удостоверяющего центра.
pub fn make_ca(cn: &str) -> Issued {
    issue(cn, None, true, None, 1)
}

/// Сертификат сервера, подписанный `ca`, с единственным SAN `DNS:<dns>`.
pub fn make_server_cert(ca: &Issued, dns: &str) -> Issued {
    issue(dns, Some(ca), false, Some(dns), 2)
}

/// Сертификат сервера, подписанный `ca`, срок действия которого истёк час назад.
pub fn make_expired_server_cert(ca: &Issued, dns: &str) -> Issued {
    let now = unix_now();
    issue_with_validity(dns, Some(ca), false, Some(dns), 4, now - 7200, now - 3600)
}

/// Сертификат сервера, подписанный `ca`, который вступит в силу только через час.
pub fn make_not_yet_valid_server_cert(ca: &Issued, dns: &str) -> Issued {
    let now = unix_now();
    issue_with_validity(dns, Some(ca), false, Some(dns), 5, now + 3600, now + 7200)
}

/// Самоподписанный сертификат сервера (не CA) с SAN `DNS:<dns>`.
pub fn make_self_signed_server(dns: &str) -> Issued {
    issue(dns, None, false, Some(dns), 3)
}

/// Заглушка сервера на 127.0.0.1; живёт, пока жив этот объект.
pub struct FakeServer {
    pub port: u16,
    /// У молчащего сервера: пока отправитель жив, соединение удерживается.
    _release: Option<mpsc::Sender<()>>,
}

fn listen() -> (TcpListener, u16) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    (listener, port)
}

/// Принимает одно соединение и молчит, пока жив `FakeServer`: клиент ждёт ответа до своего таймаута.
pub fn silent_server() -> FakeServer {
    let (listener, port) = listen();
    let (tx, rx) = mpsc::channel::<()>();
    std::thread::spawn(move || {
        let _conn = listener.accept();
        // Канал закрывается вместе с `FakeServer`.
        let _ = rx.recv();
    });
    FakeServer {
        port,
        _release: Some(tx),
    }
}

/// HTTP-сервер, который обрывает связь после приёма запроса; считает принятые соединения.
pub struct DroppingHttp {
    /// Базовый адрес вида `http://127.0.0.1:порт`.
    pub base: String,
    accepted: Arc<AtomicUsize>,
}

impl DroppingHttp {
    pub fn accepted(&self) -> usize {
        self.accepted.load(Ordering::SeqCst)
    }
}

/// Читает HTTP-запрос целиком: заголовки и тело по `Content-Length`.
fn read_http_request(sock: &mut TcpStream) {
    let _ = sock.set_read_timeout(Some(Duration::from_secs(5)));
    let mut raw = Vec::<u8>::new();
    let mut buf = [0u8; 4096];
    loop {
        let n = sock.read(&mut buf).unwrap_or(0);
        raw.extend_from_slice(&buf[..n]);
        let text = String::from_utf8_lossy(&raw).into_owned();
        if let Some((head, body)) = text.split_once("\r\n\r\n") {
            let want = head
                .lines()
                .find_map(|l| {
                    l.to_ascii_lowercase()
                        .strip_prefix("content-length: ")
                        .and_then(|v| v.trim().parse::<usize>().ok())
                })
                .unwrap_or(0);
            if body.len() >= want {
                return;
            }
        }
        if n == 0 {
            return;
        }
    }
}

/// Читает запрос целиком и закрывает соединение без ответа; с `partial_answer` — сначала отправляет начало
/// ответа с обещанным, но не отправленным телом. Так ведёт себя балансировщик, оборвавший связь после запроса.
pub fn dropping_http_server(partial_answer: bool) -> DroppingHttp {
    let (listener, port) = listen();
    let accepted = Arc::new(AtomicUsize::new(0));
    let count = accepted.clone();
    std::thread::spawn(move || {
        while let Ok((mut sock, _)) = listener.accept() {
            count.fetch_add(1, Ordering::SeqCst);
            std::thread::spawn(move || {
                read_http_request(&mut sock);
                if partial_answer {
                    let _ = sock.write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\npartial",
                    );
                }
            });
        }
    });
    DroppingHttp {
        base: format!("http://127.0.0.1:{port}"),
        accepted,
    }
}

/// Сообщение `ErrorResponse` протокола PostgreSQL: FATAL, SQLSTATE 28000.
fn error_response() -> Vec<u8> {
    fatal("28000", "harness: TLS handshake completed")
}

/// `ErrorResponse` уровня FATAL с кодом `sqlstate` и текстом `message`.
fn fatal(sqlstate: &str, message: &str) -> Vec<u8> {
    let mut fields = Vec::new();
    for (tag, value) in [
        (b'S', "FATAL"),
        (b'V', "FATAL"),
        (b'C', sqlstate),
        (b'M', message),
    ] {
        fields.push(tag);
        fields.extend_from_slice(value.as_bytes());
        fields.push(0);
    }
    fields.push(0);
    let mut msg = vec![b'E'];
    msg.extend_from_slice(&((fields.len() + 4) as u32).to_be_bytes());
    msg.extend_from_slice(&fields);
    msg
}

/// Принимает одно соединение: читает SSLRequest (8 байт), отвечает `S`, делает TLS-рукопожатие
/// сертификатом `server`, читает стартовое сообщение, отвечает `ErrorResponse` 28000 и закрывает
/// TLS. Если клиент отверг сертификат, рукопожатие у сервера просто не удаётся.
pub fn tls_server(server: &Issued) -> FakeServer {
    tls_server_replying(server, Some(error_response()))
}

/// Как `tls_server`, но после рукопожатия и стартового сообщения закрывает TLS без `ErrorResponse`: у клиента
/// ошибка без ответа сервера, и классификация идёт не через SQLSTATE.
pub fn tls_server_closing_after_handshake(server: &Issued) -> FakeServer {
    tls_server_replying(server, None)
}

fn tls_server_replying(server: &Issued, reply: Option<Vec<u8>>) -> FakeServer {
    let (listener, port) = listen();
    let mut acceptor = SslAcceptor::mozilla_intermediate_v5(SslMethod::tls()).unwrap();
    acceptor.set_private_key(&server.key).unwrap();
    acceptor.set_certificate(&server.cert).unwrap();
    acceptor.check_private_key().unwrap();
    let acceptor = acceptor.build();
    std::thread::spawn(move || {
        let Ok((mut tcp, _)) = listener.accept() else {
            return;
        };
        let _ = tcp.set_read_timeout(Some(Duration::from_secs(5)));
        let mut ssl_request = [0u8; 8];
        if tcp.read_exact(&mut ssl_request).is_err() || tcp.write_all(b"S").is_err() {
            return;
        }
        let Ok(mut tls) = acceptor.accept(tcp) else {
            return;
        };
        let mut len = [0u8; 4];
        if tls.read_exact(&mut len).is_err() {
            return;
        }
        let mut startup = vec![0u8; (u32::from_be_bytes(len) as usize).saturating_sub(4)];
        let _ = tls.read_exact(&mut startup);
        if let Some(reply) = &reply {
            let _ = tls.write_all(reply);
        }
        let _ = tls.shutdown();
    });
    FakeServer {
        port,
        _release: None,
    }
}

/// «Сервер PostgreSQL», который не отвечает на запросы. Принимает любое число соединений, на
/// стартовое сообщение отвечает `AuthenticationOk` и `ReadyForQuery` (без пароля; на `SSLRequest` —
/// `N`), дальше только читает сообщения клиента. Клиент подключается, но ответа на запрос не получит
/// никогда: так выглядит молчащий путь после успешного подключения. По счётчикам видно, закрыл ли
/// клиент сокет и отправил ли перед этим сообщение Terminate (`X`).
pub struct StallingPg {
    pub port: u16,
    accepted: Arc<AtomicUsize>,
    closed: Arc<AtomicUsize>,
    terminated: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
}

impl StallingPg {
    pub fn accepted(&self) -> usize {
        self.accepted.load(Ordering::SeqCst)
    }

    /// Сколько соединений клиент уже закрыл (сервер прочитал конец потока).
    pub fn closed(&self) -> usize {
        self.closed.load(Ordering::SeqCst)
    }

    /// Сколько соединений клиент завершил сообщением Terminate.
    pub fn terminated(&self) -> usize {
        self.terminated.load(Ordering::SeqCst)
    }

    /// Ждёт, пока клиент закроет не меньше `n` соединений, но не дольше `limit`.
    pub async fn wait_closed(&self, n: usize, limit: Duration) -> bool {
        wait_for(&self.closed, n, limit).await
    }

    /// Ждёт, пока клиент отправит Terminate не меньше `n` раз, но не дольше `limit`.
    pub async fn wait_terminated(&self, n: usize, limit: Duration) -> bool {
        wait_for(&self.terminated, n, limit).await
    }
}

async fn wait_for(counter: &AtomicUsize, n: usize, limit: Duration) -> bool {
    let deadline = Instant::now() + limit;
    while counter.load(Ordering::SeqCst) < n {
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    true
}

impl Drop for StallingPg {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

pub fn stalling_pg_server() -> StallingPg {
    pg_server(serve_stalling)
}

/// «Сервер PostgreSQL», который отвергает любой вход: на `SSLRequest` отвечает `N`, на стартовое
/// сообщение — `ErrorResponse` 28P01 (неверный пароль) и закрывает соединение. По `accepted` видно,
/// сколько попыток входа сделал клиент.
pub fn auth_failing_pg_server() -> StallingPg {
    pg_server(|mut tcp, _| {
        let _ = tcp.set_nonblocking(false);
        if read_startup(&mut tcp) {
            let _ = tcp.write_all(&fatal(
                "28P01",
                "password authentication failed for user \"u\"",
            ));
        }
    })
}

/// Читает `SSLRequest` (отвечает `N`) и стартовое сообщение клиента; `false` — клиент ушёл раньше.
fn read_startup(tcp: &mut TcpStream) -> bool {
    for _ in 0..2 {
        let mut len = [0u8; 4];
        if tcp.read_exact(&mut len).is_err() {
            return false;
        }
        let mut body = vec![0u8; (u32::from_be_bytes(len) as usize).saturating_sub(4)];
        if tcp.read_exact(&mut body).is_err() {
            return false;
        }
        // SSLRequest: код 80877103 — отказываем, клиент продолжит без TLS.
        if body == 80_877_103u32.to_be_bytes() {
            if tcp.write_all(b"N").is_err() {
                return false;
            }
            continue;
        }
        return true;
    }
    false
}

/// Слушатель на loopback: каждое соединение обслуживает `serve` в своём потоке; счётчики — в `StallingPg`.
fn pg_server(serve: fn(TcpStream, &AtomicUsize)) -> StallingPg {
    let (listener, port) = listen();
    listener.set_nonblocking(true).unwrap();
    let server = StallingPg {
        port,
        accepted: Arc::default(),
        closed: Arc::default(),
        terminated: Arc::default(),
        stop: Arc::default(),
    };
    let (accepted, closed, terminated, stop) = (
        server.accepted.clone(),
        server.closed.clone(),
        server.terminated.clone(),
        server.stop.clone(),
    );
    std::thread::spawn(move || {
        while !stop.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((tcp, _)) => {
                    accepted.fetch_add(1, Ordering::SeqCst);
                    let (closed, terminated) = (closed.clone(), terminated.clone());
                    std::thread::spawn(move || {
                        serve(tcp, &terminated);
                        closed.fetch_add(1, Ordering::SeqCst);
                    });
                }
                Err(_) => std::thread::sleep(Duration::from_millis(10)),
            }
        }
    });
    server
}

/// Одно соединение: рукопожатие, затем чтение сообщений клиента до конца потока (Terminate считается).
fn serve_stalling(mut tcp: TcpStream, terminated: &AtomicUsize) {
    // Принятый сокет наследует неблокирующий режим слушателя (macOS, BSD).
    let _ = tcp.set_nonblocking(false);
    let _ = tcp.set_nodelay(true);
    if !read_startup(&mut tcp) {
        return;
    }
    let mut reply = Vec::new();
    reply.extend_from_slice(&[b'R', 0, 0, 0, 8, 0, 0, 0, 0]);
    reply.extend_from_slice(&[b'K', 0, 0, 0, 12, 0, 0, 0, 1, 0, 0, 0, 2]);
    reply.extend_from_slice(&[b'Z', 0, 0, 0, 5, b'I']);
    if tcp.write_all(&reply).is_err() {
        return;
    }
    // Сообщение клиента: тип (1 байт), длина вместе с самой собой (4 байта), содержимое.
    loop {
        let mut tag = [0u8; 1];
        let mut len = [0u8; 4];
        if tcp.read_exact(&mut tag).is_err() || tcp.read_exact(&mut len).is_err() {
            return;
        }
        let mut body = vec![0u8; (u32::from_be_bytes(len) as usize).saturating_sub(4)];
        if tcp.read_exact(&mut body).is_err() {
            return;
        }
        if tag[0] == b'X' {
            terminated.fetch_add(1, Ordering::SeqCst);
        }
    }
}

/// Запросы CONNECT, которые получил поддельный SOCKS5-прокси: тип адреса (3 — имя, 1 — IPv4), узел, порт.
pub type SocksRequests = Arc<std::sync::Mutex<Vec<(u8, String, u16)>>>;

/// Поддельный SOCKS5-прокси без аутентификации: любой CONNECT (к имени или IPv4) → TCP к `upstream`, затем
/// прокачка байтов. Запросы запоминаются: по ним видно, что имя узла разрешает прокси, а не клиент.
pub async fn socks_proxy_recording(upstream: (&str, u16)) -> (std::net::SocketAddr, SocksRequests) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let requests: SocksRequests = Arc::default();
    let upstream = format!("{}:{}", upstream.0, upstream.1);
    let seen = requests.clone();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = l.accept().await {
            let (upstream, seen) = (upstream.clone(), seen.clone());
            tokio::spawn(async move {
                let mut buf = [0u8; 262];
                let request = async {
                    s.read_exact(&mut buf[..2]).await?;
                    let n = buf[1] as usize;
                    s.read_exact(&mut buf[..n]).await?;
                    s.write_all(&[5, 0]).await?;
                    s.read_exact(&mut buf[..4]).await?;
                    let atyp = buf[3];
                    let host = match atyp {
                        3 => {
                            s.read_exact(&mut buf[..1]).await?;
                            let n = buf[0] as usize;
                            s.read_exact(&mut buf[..n]).await?;
                            String::from_utf8_lossy(&buf[..n]).into_owned()
                        }
                        _ => {
                            s.read_exact(&mut buf[..4]).await?;
                            format!("{}.{}.{}.{}", buf[0], buf[1], buf[2], buf[3])
                        }
                    };
                    s.read_exact(&mut buf[..2]).await?;
                    let port = u16::from_be_bytes([buf[0], buf[1]]);
                    Ok::<_, std::io::Error>((atyp, host, port))
                };
                let Ok(req) = request.await else { return };
                seen.lock().unwrap().push(req);
                let Ok(mut up) = tokio::net::TcpStream::connect(upstream.as_str()).await else {
                    return;
                };
                if s.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]).await.is_ok() {
                    let _ = tokio::io::copy_bidirectional(&mut s, &mut up).await;
                }
            });
        }
    });
    (addr, requests)
}

/// То же без журнала запросов.
pub async fn socks_proxy_to(upstream: (&str, u16)) -> std::net::SocketAddr {
    socks_proxy_recording(upstream).await.0
}
