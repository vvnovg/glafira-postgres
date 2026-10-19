//! TLS для PostgreSQL и Patroni API: CA, клиентский сертификат (PEM / PKCS#12), системные корневые
//! сертификаты. Файлы читаются при каждом создании пула или клиента — ротация подхватывается.

use std::cell::Cell;
use std::collections::HashSet;
use std::sync::OnceLock;

use openssl::error::ErrorStack;
use openssl::pkcs12::{ParsedPkcs12_2, Pkcs12};
use openssl::pkey::{PKey, Private};
use openssl::provider::Provider;
use openssl::ssl::{SslConnector, SslConnectorBuilder, SslMethod, SslVerifyMode};
use openssl::x509::X509;
use openssl::x509::store::{X509Store, X509StoreBuilder};

use crate::conn::errors::tls_setup;
use crate::conn::profile::{CertFormat, ClientCert, SslMode, is_network_or_device_path};
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
    // Профиль проверен при сохранении и импорте (`validate_profile`), но файл connections.json старой версии
    // мог сохранить такой путь: сетевой ресурс не открывается и при подключении.
    if is_network_or_device_path(path) {
        return Err(app_err!(
            ErrorCode::InvalidInput,
            "errors.connections.pathNetwork",
            "Certificate and key files must be on this computer, not on a network share or a device path: {{path}}",
            path = path
        ));
    }
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

/// Закрытый ключ из PEM. Без пароля OpenSSL по умолчанию спросил бы его в терминале (`/dev/tty`),
/// поэтому ставится свой обратный вызов, который отказывает: зашифрованный ключ без пароля — ошибка.
fn load_pem_key(pem: &[u8], password: Option<&str>) -> Result<PKey<Private>, String> {
    match password.filter(|p| !p.is_empty()) {
        Some(p) => {
            PKey::private_key_from_pem_passphrase(pem, p.as_bytes()).map_err(|e| e.to_string())
        }
        None => {
            let asked = Cell::new(false);
            PKey::private_key_from_pem_callback(pem, |_buf| {
                asked.set(true);
                Err(ErrorStack::get())
            })
            .map_err(|e| {
                if asked.get() {
                    "the private key is encrypted: a password is required".to_string()
                } else {
                    e.to_string()
                }
            })
        }
    }
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
            let key = load_pem_key(&key_pem, password).map_err(|e| bad_file(key_path, e))?;
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

/// Системные корни в DER — для клиентов, которым нельзя отдать «системное» хранилище целиком
/// (reqwest без проверки имени узла).
pub fn system_roots_der() -> Vec<Vec<u8>> {
    system_roots()
        .iter()
        .filter_map(|c| c.to_der().ok())
        .collect()
}

/// Содержимое CA-файла и его сертификаты; в файле должен быть хотя бы один сертификат.
fn read_ca(path: &str) -> AppResult<(Vec<u8>, Vec<X509>)> {
    let data = read(path)?;
    let certs = X509::stack_from_pem(&data).map_err(|e| bad_file(path, e))?;
    if certs.is_empty() {
        return Err(bad_file(path, "no certificates"));
    }
    Ok((data, certs))
}

/// CA-файл как PEM-набор (для reqwest); проверяется, что в нём есть хотя бы один сертификат.
pub fn ca_pem_bundle(path: &str) -> AppResult<Vec<u8>> {
    read_ca(path).map(|(data, _)| data)
}

/// Хранилище доверия только из сертификатов CA-файла: системные корни и пути OpenSSL по умолчанию
/// (`SSL_CERT_FILE`, `SSL_CERT_DIR`, openssldir) в него не попадают. Повторы в файле пропускаются,
/// остальные ошибки добавления — это неверный файл.
fn pinned_store(path: &str) -> AppResult<X509Store> {
    let (_, certs) = read_ca(path)?;
    let mut store = X509StoreBuilder::new().map_err(tls_setup)?;
    let mut seen = HashSet::new();
    for c in certs {
        let der = c.to_der().map_err(|e| bad_file(path, e))?;
        if seen.insert(der) {
            store.add_cert(c).map_err(|e| bad_file(path, e))?;
        }
    }
    Ok(store.build())
}

/// Кому доверять: указанный CA-файл заменяет все корни; без него — системные, но только если
/// режим проверяет сертификат (`verify-ca`, `verify-full`).
fn install_trust(
    b: &mut SslConnectorBuilder,
    mode: SslMode,
    ca_path: Option<&str>,
) -> AppResult<()> {
    match ca_path {
        Some(p) => b.set_cert_store(pinned_store(p)?),
        None if matches!(mode, SslMode::VerifyCa | SslMode::VerifyFull) => {
            for c in system_roots() {
                // Один негодный системный корень не должен ломать подключение: остальные доверенные.
                let _ = b.cert_store_mut().add_cert(c.clone());
            }
        }
        None => {}
    }
    Ok(())
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
    install_trust(&mut b, mode, ca_path)?;
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
    use crate::conn::testutil;
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
    fn network_and_device_paths_are_never_opened() {
        for path in [
            r"\\glafira-test.invalid\share\ca.pem",
            "//glafira-test.invalid/share/ca.pem",
            r"\\?\UNC\glafira-test.invalid\share\ca.pem",
            r"\\.\pipe\x",
            r"\??\UNC\glafira-test.invalid\share\ca.pem",
        ] {
            let cc = ClientCert {
                format: CertFormat::Pem,
                certificate_path: path.into(),
                key_path: Some(path.into()),
            };
            assert_eq!(
                load_identity(&cc, None).unwrap_err().key,
                "errors.connections.pathNetwork",
                "{path}"
            );
            assert_eq!(
                ca_pem_bundle(path).unwrap_err().key,
                "errors.connections.pathNetwork",
                "{path}"
            );
            assert_eq!(
                pg_connector(SslMode::VerifyCa, Some(path), None, None)
                    .unwrap_err()
                    .key,
                "errors.connections.pathNetwork",
                "{path}"
            );
        }
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
    fn encrypted_pem_key_without_password_is_rejected_without_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let (cert, key) = self_signed();
        let cert_path = write(dir.path(), "c.pem", &cert.to_pem().unwrap());
        let enc = write(
            dir.path(),
            "ke.pem",
            &key.private_key_to_pem_pkcs8_passphrase(Cipher::aes_256_cbc(), b"pw")
                .unwrap(),
        );
        let cc = ClientCert {
            format: CertFormat::Pem,
            certificate_path: cert_path,
            key_path: Some(enc),
        };
        // Пароля нет или он пустой: OpenSSL не должен спрашивать его в терминале.
        for password in [None, Some("")] {
            let e = load_identity(&cc, password).unwrap_err();
            assert_eq!(e.key, "errors.tls.badFile", "{password:?}");
            assert!(
                e.details.unwrap().contains("password is required"),
                "{password:?}"
            );
        }
    }

    fn store_len(c: &SslConnector) -> usize {
        c.context().cert_store().all_certificates().len()
    }

    #[test]
    fn pinned_ca_replaces_default_roots() {
        let dir = tempfile::tempdir().unwrap();
        let pinned = testutil::make_ca("pinned");
        let default_root = testutil::make_ca("default-root");
        let path = write(dir.path(), "ca.pem", &pinned.cert.to_pem().unwrap());
        for mode in [SslMode::VerifyCa, SslMode::VerifyFull] {
            let mut b = SslConnector::builder(SslMethod::tls_client()).unwrap();
            // Корень «по умолчанию» (как из SSL_CERT_FILE): после установки CA его в хранилище быть не должно.
            b.cert_store_mut()
                .add_cert(default_root.cert.clone())
                .unwrap();
            install_trust(&mut b, mode, Some(&path)).unwrap();
            let connector = b.build();
            let certs = connector.context().cert_store().all_certificates();
            let ders: Vec<Vec<u8>> = certs.iter().map(|c| c.to_der().unwrap()).collect();
            assert!(
                ders == vec![pinned.cert.to_der().unwrap()],
                "{mode:?}: в хранилище {} сертификатов вместо одного (закреплённого)",
                ders.len()
            );
        }
    }

    #[test]
    fn pinned_bundle_tolerates_duplicates_but_not_junk() {
        let dir = tempfile::tempdir().unwrap();
        let ca = testutil::make_ca("pinned");
        let pem = ca.cert.to_pem().unwrap();
        let twice = write(dir.path(), "two.pem", &[pem.clone(), pem].concat());
        let c = pg_connector(SslMode::VerifyFull, Some(&twice), None, None)
            .unwrap()
            .unwrap();
        assert_eq!(store_len(&c), 1);
        let empty = write(dir.path(), "empty.pem", b"");
        assert_eq!(
            pg_connector(SslMode::VerifyFull, Some(&empty), None, None)
                .unwrap_err()
                .key,
            "errors.tls.badFile"
        );
    }

    #[test]
    fn system_roots_only_when_the_mode_verifies() {
        let baseline = store_len(
            &SslConnector::builder(SslMethod::tls_client())
                .unwrap()
                .build(),
        );
        for mode in [SslMode::Prefer, SslMode::Require] {
            let c = pg_connector(mode, None, None, None).unwrap().unwrap();
            assert_eq!(store_len(&c), baseline, "{mode:?}");
        }
        for mode in [SslMode::VerifyCa, SslMode::VerifyFull] {
            let c = pg_connector(mode, None, None, None).unwrap().unwrap();
            assert!(store_len(&c) >= baseline, "{mode:?}");
        }
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
