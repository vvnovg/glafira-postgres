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
    // Начало — с запасом назад: часы тестовой машины могут чуть расходиться.
    b.set_not_before(&Asn1Time::from_unix(unix_now() - 3600).unwrap())
        .unwrap();
    b.set_not_after(&Asn1Time::days_from_now(2).unwrap())
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
        let _ = tls.write_all(&error_response());
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
