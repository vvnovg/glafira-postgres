//! TLS для PostgreSQL и Patroni API: CA, клиентский сертификат (PEM / PKCS#12), системные корневые
//! сертификаты. Файлы читаются при каждом создании пула или клиента — ротация подхватывается.

use std::sync::OnceLock;

use openssl::error::ErrorStack;
use openssl::pkcs12::{ParsedPkcs12_2, Pkcs12};
use openssl::pkey::{PKey, Private};
use openssl::provider::Provider;
use openssl::ssl::{SslConnector, SslMethod, SslVerifyMode};
use openssl::x509::X509;

use crate::conn::errors::tls_setup;
use crate::conn::profile::{CertFormat, ClientCert, SslMode};
use crate::error::{AppError, AppResult, ErrorCode};

pub struct Identity {
    pub cert: X509,
    pub chain: Vec<X509>,
    pub key: PKey<Private>,
}

// Ключ в отладочный вывод не попадает; Debug нужен для `unwrap_err()` в тестах.
impl std::fmt::Debug for Identity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Identity")
            .field("chain_len", &self.chain.len())
            .finish_non_exhaustive()
    }
}

fn read(path: &str) -> AppResult<Vec<u8>> {
    std::fs::read(path).map_err(|e| {
        app_err!(
            ErrorCode::InvalidInput,
            "errors.tls.readFailed",
            "Cannot read {{path}}",
            path = path
        )
        .with_details(e.to_string())
    })
}

fn bad_file(path: &str, e: impl std::fmt::Display) -> AppError {
    app_err!(
        ErrorCode::InvalidInput,
        "errors.tls.badFile",
        "{{path}} is not a valid certificate or key, or the password is wrong",
        path = path
    )
    .with_details(e.to_string())
}

static LEGACY: OnceLock<Option<Provider>> = OnceLock::new();

/// Старые PKCS#12 (RC2/3DES) требуют провайдер legacy OpenSSL 3 — как в Glafira for Kafka.
fn parse_pkcs12(p12: &Pkcs12, password: &str) -> Result<ParsedPkcs12_2, ErrorStack> {
    match p12.parse2(password) {
        Err(e)
            if is_unsupported(&e)
                && LEGACY
                    .get_or_init(|| Provider::try_load(None, "legacy", true).ok())
                    .is_some() =>
        {
            p12.parse2(password)
        }
        r => r,
    }
}

fn is_unsupported(e: &ErrorStack) -> bool {
    let t = e.to_string().to_lowercase();
    t.contains("unsupported") || t.contains("rc2") || t.contains("fetch")
}

pub fn load_identity(c: &ClientCert, password: Option<&str>) -> AppResult<Identity> {
    match c.format {
        CertFormat::Pem => {
            let data = read(&c.certificate_path)?;
            let mut certs = X509::stack_from_pem(&data)
                .map_err(|e| bad_file(&c.certificate_path, e))?
                .into_iter();
            let cert = certs
                .next()
                .ok_or_else(|| bad_file(&c.certificate_path, "no certificate"))?;
            let key_path = c.key_path.as_deref().unwrap_or(&c.certificate_path);
            let key_pem = read(key_path)?;
            let key = match password.filter(|p| !p.is_empty()) {
                Some(p) => PKey::private_key_from_pem_passphrase(&key_pem, p.as_bytes()),
                None => PKey::private_key_from_pem(&key_pem),
            }
            .map_err(|e| bad_file(key_path, e))?;
            Ok(Identity {
                cert,
                chain: certs.collect(),
                key,
            })
        }
        CertFormat::Pkcs12 => {
            let path = &c.certificate_path;
            let p12 = Pkcs12::from_der(&read(path)?).map_err(|e| bad_file(path, e))?;
            let parsed =
                parse_pkcs12(&p12, password.unwrap_or("")).map_err(|e| bad_file(path, e))?;
            let cert = parsed
                .cert
                .ok_or_else(|| bad_file(path, "no certificate"))?;
            let key = parsed
                .pkey
                .ok_or_else(|| bad_file(path, "no private key"))?;
            let chain = parsed
                .ca
                .map(|s| s.into_iter().collect())
                .unwrap_or_default();
            Ok(Identity { cert, chain, key })
        }
    }
}

static SYSTEM_ROOTS: OnceLock<Vec<X509>> = OnceLock::new();

/// Системные корневые сертификаты (Keychain, хранилище Windows, /etc/ssl): vendored OpenSSL путей ОС не знает.
fn system_roots() -> &'static [X509] {
    SYSTEM_ROOTS.get_or_init(|| {
        let res = rustls_native_certs::load_native_certs();
        for e in &res.errors {
            tracing::warn!("system root certificates: {e}");
        }
        res.certs
            .iter()
            .filter_map(|c| X509::from_der(c.as_ref()).ok())
            .collect()
    })
}

/// CA-файл как PEM-набор (для reqwest); проверяется, что в нём есть хотя бы один сертификат.
pub fn ca_pem_bundle(path: &str) -> AppResult<Vec<u8>> {
    let data = read(path)?;
    let certs = X509::stack_from_pem(&data).map_err(|e| bad_file(path, e))?;
    if certs.is_empty() {
        return Err(bad_file(path, "no certificates"));
    }
    Ok(data)
}

/// `SslConnector` для режима `mode`; `None` — без TLS (`disable`).
pub fn pg_connector(
    mode: SslMode,
    ca_path: Option<&str>,
    cert: Option<&ClientCert>,
    key_password: Option<&str>,
) -> AppResult<Option<SslConnector>> {
    if mode == SslMode::Disable {
        return Ok(None);
    }
    let mut b = SslConnector::builder(SslMethod::tls_client()).map_err(tls_setup)?;
    let roots: Vec<X509> = match ca_path {
        Some(p) => {
            let data = ca_pem_bundle(p)?;
            X509::stack_from_pem(&data).map_err(|e| bad_file(p, e))?
        }
        None => system_roots().to_vec(),
    };
    for c in roots {
        // Повтор сертификата в хранилище — не ошибка.
        let _ = b.cert_store_mut().add_cert(c);
    }
    if let Some(c) = cert {
        let id = load_identity(c, key_password)?;
        b.set_certificate(&id.cert).map_err(tls_setup)?;
        b.set_private_key(&id.key).map_err(tls_setup)?;
        for extra in id.chain {
            b.add_extra_chain_cert(extra).map_err(tls_setup)?;
        }
    }
    b.set_verify(match mode {
        SslMode::VerifyCa | SslMode::VerifyFull => SslVerifyMode::PEER,
        _ => SslVerifyMode::NONE,
    });
    Ok(Some(b.build()))
}

/// Сертификат с цепочкой и незашифрованный ключ PKCS#8 одним PEM — для `reqwest::Identity::from_pem`.
pub fn identity_pem(c: &ClientCert, password: Option<&str>) -> AppResult<Vec<u8>> {
    let id = load_identity(c, password)?;
    let mut out = id.cert.to_pem().map_err(tls_setup)?;
    for x in &id.chain {
        out.extend(x.to_pem().map_err(tls_setup)?);
    }
    out.extend(id.key.private_key_to_pem_pkcs8().map_err(tls_setup)?);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use openssl::asn1::Asn1Time;
    use openssl::hash::MessageDigest;
    use openssl::symm::Cipher;
    use openssl::x509::X509NameBuilder;

    fn self_signed() -> (X509, PKey<Private>) {
        let key = PKey::from_rsa(openssl::rsa::Rsa::generate(2048).unwrap()).unwrap();
        let mut name = X509NameBuilder::new().unwrap();
        name.append_entry_by_text("CN", "glafira-test").unwrap();
        let name = name.build();
        let mut b = X509::builder().unwrap();
        b.set_version(2).unwrap();
        b.set_subject_name(&name).unwrap();
        b.set_issuer_name(&name).unwrap();
        b.set_pubkey(&key).unwrap();
        b.set_not_before(&Asn1Time::days_from_now(0).unwrap())
            .unwrap();
        b.set_not_after(&Asn1Time::days_from_now(1).unwrap())
            .unwrap();
        b.sign(&key, MessageDigest::sha256()).unwrap();
        (b.build(), key)
    }

    fn write(dir: &std::path::Path, name: &str, data: &[u8]) -> String {
        let p = dir.join(name);
        std::fs::write(&p, data).unwrap();
        p.to_string_lossy().into_owned()
    }

    #[test]
    fn pem_identity_plain_and_encrypted() {
        let dir = tempfile::tempdir().unwrap();
        let (cert, key) = self_signed();
        let cert_path = write(dir.path(), "c.pem", &cert.to_pem().unwrap());
        let plain = write(
            dir.path(),
            "k.pem",
            &key.private_key_to_pem_pkcs8().unwrap(),
        );
        let enc = write(
            dir.path(),
            "ke.pem",
            &key.private_key_to_pem_pkcs8_passphrase(Cipher::aes_256_cbc(), b"pw")
                .unwrap(),
        );
        let cc = |k: &str| ClientCert {
            format: CertFormat::Pem,
            certificate_path: cert_path.clone(),
            key_path: Some(k.into()),
        };
        assert!(load_identity(&cc(&plain), None).is_ok());
        assert!(load_identity(&cc(&enc), Some("pw")).is_ok());
        assert_eq!(
            load_identity(&cc(&enc), Some("wrong")).unwrap_err().key,
            "errors.tls.badFile"
        );
    }

    #[test]
    fn pkcs12_identity() {
        let dir = tempfile::tempdir().unwrap();
        let (cert, key) = self_signed();
        let p12 = Pkcs12::builder()
            .name("x")
            .pkey(&key)
            .cert(&cert)
            .build2("pw")
            .unwrap();
        let path = write(dir.path(), "c.p12", &p12.to_der().unwrap());
        let cc = ClientCert {
            format: CertFormat::Pkcs12,
            certificate_path: path,
            key_path: None,
        };
        assert!(load_identity(&cc, Some("pw")).is_ok());
        assert_eq!(
            load_identity(&cc, Some("nope")).unwrap_err().key,
            "errors.tls.badFile"
        );
    }

    #[test]
    fn missing_file_is_reported_with_path() {
        let cc = ClientCert {
            format: CertFormat::Pem,
            certificate_path: "/nope/c.pem".into(),
            key_path: Some("/nope/k.pem".into()),
        };
        let e = load_identity(&cc, None).unwrap_err();
        assert_eq!(e.key, "errors.tls.readFailed");
        assert_eq!(e.params["path"], "/nope/c.pem");
    }

    #[test]
    fn connector_follows_sslmode() {
        assert!(
            pg_connector(SslMode::Disable, None, None, None)
                .unwrap()
                .is_none()
        );
        assert!(
            pg_connector(SslMode::Prefer, None, None, None)
                .unwrap()
                .is_some()
        );
        let dir = tempfile::tempdir().unwrap();
        let (cert, _) = self_signed();
        let ca = write(dir.path(), "ca.pem", &cert.to_pem().unwrap());
        assert!(
            pg_connector(SslMode::VerifyFull, Some(&ca), None, None)
                .unwrap()
                .is_some()
        );
        let junk = write(dir.path(), "junk.pem", b"not a certificate");
        assert_eq!(
            pg_connector(SslMode::VerifyCa, Some(&junk), None, None)
                .unwrap_err()
                .key,
            "errors.tls.badFile"
        );
    }

    #[test]
    fn identity_pem_has_certificate_and_unencrypted_key() {
        let dir = tempfile::tempdir().unwrap();
        let (cert, key) = self_signed();
        let p12 = Pkcs12::builder()
            .name("x")
            .pkey(&key)
            .cert(&cert)
            .build2("pw")
            .unwrap();
        let path = write(dir.path(), "c.p12", &p12.to_der().unwrap());
        let pem = identity_pem(
            &ClientCert {
                format: CertFormat::Pkcs12,
                certificate_path: path,
                key_path: None,
            },
            Some("pw"),
        )
        .unwrap();
        let text = String::from_utf8(pem).unwrap();
        assert!(text.contains("BEGIN CERTIFICATE"));
        assert!(text.contains("BEGIN PRIVATE KEY"));
    }
}
