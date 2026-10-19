//! Помощники тестов TLS без Docker: тестовые CA и сертификаты, «сервер PostgreSQL» на loopback.
//! Сервер отвечает на SSLRequest, делает TLS-рукопожатие и после стартового сообщения клиента
//! отдаёт `ErrorResponse` 28000 — по нему тест видит, что рукопожатие прошло.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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
    let mut fields = Vec::new();
    for (tag, value) in [
        (b'S', "FATAL"),
        (b'V', "FATAL"),
        (b'C', "28000"),
        (b'M', "harness: TLS handshake completed"),
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
