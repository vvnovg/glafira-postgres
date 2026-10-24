//! Поддельный OpenSSH для unit-тестов туннеля: разбирает те же аргументы, что настоящий ssh, держит SOCKS5 на
//! порту из `-D` и ведёт себя по `FAKE_SSH_MODE`: `ok` (по умолчанию), `auth`, `hostkey`. Переменные:
//! `FAKE_SSH_UPSTREAM=host:port` — куда идут все CONNECT (иначе — к запрошенному адресу);
//! `FAKE_SSH_DIE_MS=N` — завершиться с кодом 255 через N мс после готовности; `FAKE_SSH_VERSION` — ответ на `-V`;
//! `FAKE_SSH_BIND_FAIL_ONCE=<файл-маркер>` — первый запуск падает «Address already in use»;
//! `FAKE_SSH_AUTH_FAIL_IF=<файл>` — пока файл есть, вход отклоняется (как `auth`): перезапуски туннеля не удаются.
//! Режим `hostkey`: отпечаток `SHA256:fake`; с `GLAFIRA_ASKPASS_TRUST=SHA256:fake` ведёт себя как `ok`, иначе пишет
//! запрос в `GLAFIRA_ASKPASS_REPORT` и падает «Host key verification failed.» — как настоящий ssh после ответа
//! askpass «no».
//! Недоступный узел — как у настоящего `ssh -D`: успех SOCKS5, затем закрытие соединения (канал к узлу не открылся).
//!
//! Собирается `cargo test` вместе с примерами; `cargo test --lib` примеры не собирает — тогда unit-тесты
//! компилируют этот файл сами (`net::ssh::testutil::fake_ssh_bin`), поэтому здесь только `std`.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-V") {
        eprintln!(
            "{}",
            std::env::var("FAKE_SSH_VERSION").unwrap_or_else(|_| "OpenSSH_9.9p1 fake".into())
        );
        return;
    }
    let socks = args
        .windows(2)
        .find(|w| w[0] == "-D")
        .map(|w| w[1].clone())
        .expect("-D 127.0.0.1:port");
    let refuse =
        std::env::var("FAKE_SSH_AUTH_FAIL_IF").is_ok_and(|f| std::path::Path::new(&f).exists());
    let mode = if refuse {
        "auth".to_string()
    } else {
        std::env::var("FAKE_SSH_MODE").unwrap_or_else(|_| "ok".into())
    };
    match mode.as_str() {
        "auth" => {
            eprintln!("tester@bastion: Permission denied (publickey,password).");
            std::process::exit(255);
        }
        "hostkey" if std::env::var("GLAFIRA_ASKPASS_TRUST").as_deref() != Ok("SHA256:fake") => {
            let prompt = "The authenticity of host 'bastion (10.0.0.5)' can't be established.\nED25519 key fingerprint is SHA256:fake.\nAre you sure you want to continue connecting (yes/no/[fingerprint])?";
            if let Ok(report) = std::env::var("GLAFIRA_ASKPASS_REPORT") {
                std::fs::write(report, prompt).unwrap();
            }
            eprintln!("Host key verification failed.");
            std::process::exit(255);
        }
        _ => {}
    }
    if let Ok(marker) = std::env::var("FAKE_SSH_BIND_FAIL_ONCE")
        && !std::path::Path::new(&marker).exists()
    {
        std::fs::write(&marker, b"1").unwrap();
        eprintln!(
            "bind [127.0.0.1]:{}: Address already in use",
            socks.rsplit(':').next().unwrap()
        );
        eprintln!("Could not request local forwarding.");
        std::process::exit(255);
    }
    let listener = TcpListener::bind(&socks).expect("bind -D port");
    if let Ok(ms) = std::env::var("FAKE_SSH_DIE_MS") {
        let ms: u64 = ms.parse().unwrap();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(ms));
            eprintln!("Connection to bastion closed by remote host.");
            std::process::exit(255);
        });
    }
    for client in listener.incoming().flatten() {
        std::thread::spawn(move || {
            // Проба готовности (соединение без единого байта) и оборванные клиенты — не ошибка ssh: молча.
            let _ = serve(client);
        });
    }
}

/// SOCKS5 без аутентификации: CONNECT к имени (0x03) или IPv4 (0x01).
fn serve(mut c: TcpStream) -> std::io::Result<()> {
    let mut b = [0u8; 300];
    c.read_exact(&mut b[..2])?;
    let n = b[1] as usize;
    c.read_exact(&mut b[..n])?;
    c.write_all(&[5, 0])?;
    c.read_exact(&mut b[..4])?;
    let target = match b[3] {
        3 => {
            c.read_exact(&mut b[..1])?;
            let n = b[0] as usize;
            c.read_exact(&mut b[..n])?;
            let host = String::from_utf8_lossy(&b[..n]).into_owned();
            c.read_exact(&mut b[..2])?;
            format!("{host}:{}", u16::from_be_bytes([b[0], b[1]]))
        }
        1 => {
            c.read_exact(&mut b[..6])?;
            format!(
                "{}.{}.{}.{}:{}",
                b[0],
                b[1],
                b[2],
                b[3],
                u16::from_be_bytes([b[4], b[5]])
            )
        }
        _ => {
            c.write_all(&[5, 8, 0, 1, 0, 0, 0, 0, 0, 0])?;
            return Ok(());
        }
    };
    // Как `ssh -D`: успех отвечается до открытия канала к узлу.
    c.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0])?;
    let upstream = std::env::var("FAKE_SSH_UPSTREAM").unwrap_or(target);
    let Ok(mut up) = TcpStream::connect(&upstream) else {
        // Бастион не видит узел: канал не открылся, ssh закрывает соединение (клиент видит конец потока).
        return Ok(());
    };
    let mut c2 = c.try_clone()?;
    let mut up2 = up.try_clone()?;
    std::thread::spawn(move || {
        let _ = std::io::copy(&mut c2, &mut up2);
        let _ = up2.shutdown(std::net::Shutdown::Write);
    });
    let _ = std::io::copy(&mut up, &mut c);
    let _ = c.shutdown(std::net::Shutdown::Write);
    Ok(())
}
