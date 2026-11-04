//! Перенос из Glafira for Kafka (crates/glafira-mcp/src/endpoint.rs): адрес — перечисление `Endpoint` вместо строки,
//! имя запасного сокета и pipe — от slug приложения, случайная часть имени pipe — здесь, адрес чужой ОС — ошибка
//! `Unsupported`.
//! Где слушает приложение и как к нему подключается шим (спец. P4 §3.3): Unix socket на macOS и Linux, named pipe на
//! Windows. В `mcp.json` адрес пишется строкой.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use tokio::io::{AsyncRead, AsyncWrite};

pub trait Stream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Stream for T {}
pub type BoxStream = Box<dyn Stream>;

pub const SOCKET_NAME: &str = "mcp.sock";
/// Предел `sun_path` — 104 байта на macOS и 108 на Linux; берём с запасом.
pub const MAX_SOCKET_PATH: usize = 100;
pub const PIPE_PREFIX: &str = r"\\.\pipe\";

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Endpoint {
    /// Путь Unix socket (macOS, Linux).
    Unix(PathBuf),
    /// Полное имя named pipe `\\.\pipe\…` (Windows).
    Pipe(String),
}

impl Endpoint {
    /// Адрес нового сервера: `<каталог данных>/mcp.sock`, если путь не длиннее 100 байт, иначе
    /// `$XDG_RUNTIME_DIR/<slug>-mcp.sock` или `<временный каталог>/<slug>-mcp.sock`.
    #[cfg(unix)]
    pub fn for_app(data_dir: &Path, slug: &str) -> Endpoint {
        let p = data_dir.join(SOCKET_NAME);
        if p.as_os_str().len() <= MAX_SOCKET_PATH {
            return Endpoint::Unix(p);
        }
        let base = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .filter(|d| d.is_dir())
            .unwrap_or_else(std::env::temp_dir);
        Endpoint::Unix(base.join(format!("{slug}-mcp.sock")))
    }

    /// Адрес нового сервера: `\\.\pipe\<slug>-mcp-<16 hex>`, новая случайная часть на каждый запуск.
    #[cfg(windows)]
    pub fn for_app(_data_dir: &Path, slug: &str) -> Endpoint {
        let suffix = uuid::Uuid::new_v4().simple().to_string();
        Endpoint::Pipe(format!("{PIPE_PREFIX}{slug}-mcp-{}", &suffix[..16]))
    }

    /// Адрес из строки `mcp.json`: имя pipe по префиксу `\\.\pipe\`, иначе путь сокета.
    pub fn parse(s: &str) -> Endpoint {
        if s.starts_with(PIPE_PREFIX) {
            Endpoint::Pipe(s.to_string())
        } else {
            Endpoint::Unix(PathBuf::from(s))
        }
    }
}

impl fmt::Display for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Endpoint::Unix(p) => write!(f, "{}", p.display()),
            Endpoint::Pipe(name) => f.write_str(name),
        }
    }
}

impl Serialize for Endpoint {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Endpoint {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(Endpoint::parse(&s))
    }
}

/// Адрес другой ОС (pipe на Unix, сокет на Windows).
fn unsupported(ep: &Endpoint) -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        format!("MCP endpoint {ep} is not supported on this OS"),
    )
}

pub use imp::{Listener, connect};

#[cfg(unix)]
mod imp {
    use std::io;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    use tokio::net::{UnixListener, UnixStream};

    use super::{BoxStream, Endpoint, unsupported};

    pub struct Listener {
        inner: UnixListener,
        path: PathBuf,
    }

    impl Listener {
        /// Живой сокет по этому пути — ошибка `AddrInUse`; оставшийся после сбоя файл удаляется.
        pub async fn bind(ep: &Endpoint) -> io::Result<Self> {
            let Endpoint::Unix(path) = ep else {
                return Err(unsupported(ep));
            };
            if path.exists() {
                if UnixStream::connect(path).await.is_ok() {
                    return Err(io::Error::new(
                        io::ErrorKind::AddrInUse,
                        "the MCP endpoint is in use",
                    ));
                }
                let _ = std::fs::remove_file(path);
            }
            let inner = UnixListener::bind(path)?;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
            Ok(Self {
                inner,
                path: path.clone(),
            })
        }

        pub async fn accept(&mut self) -> io::Result<BoxStream> {
            let (s, _) = self.inner.accept().await?;
            Ok(Box::new(s))
        }
    }

    impl Drop for Listener {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    pub async fn connect(ep: &Endpoint) -> io::Result<BoxStream> {
        let Endpoint::Unix(path) = ep else {
            return Err(unsupported(ep));
        };
        Ok(Box::new(UnixStream::connect(path).await?))
    }
}

#[cfg(windows)]
mod imp {
    use std::io;
    use std::time::Duration;

    use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeServer, ServerOptions};

    use super::{BoxStream, Endpoint, unsupported};

    /// ERROR_PIPE_BUSY: все экземпляры pipe заняты, сервер вот-вот создаст следующий.
    const ERROR_PIPE_BUSY: i32 = 231;

    pub struct Listener {
        name: String,
        next: NamedPipeServer,
    }

    impl Listener {
        pub async fn bind(ep: &Endpoint) -> io::Result<Self> {
            let Endpoint::Pipe(name) = ep else {
                return Err(unsupported(ep));
            };
            let next = ServerOptions::new()
                .first_pipe_instance(true)
                .reject_remote_clients(true)
                .create(name)?;
            Ok(Self {
                name: name.clone(),
                next,
            })
        }

        fn create(&self) -> io::Result<NamedPipeServer> {
            ServerOptions::new()
                .reject_remote_clients(true)
                .create(&self.name)
        }

        pub async fn accept(&mut self) -> io::Result<BoxStream> {
            if let Err(e) = self.next.connect().await {
                // Экземпляр после неудачного connect (ERROR_NO_DATA: клиент уже отключился и т. п.)
                // больше не примет соединение — иначе каждый следующий accept падал бы сразу.
                self.next = self.create()?;
                return Err(e);
            }
            let fresh = self.create()?;
            Ok(Box::new(std::mem::replace(&mut self.next, fresh)))
        }
    }

    pub async fn connect(ep: &Endpoint) -> io::Result<BoxStream> {
        let Endpoint::Pipe(name) = ep else {
            return Err(unsupported(ep));
        };
        for _ in 0..20 {
            match ClientOptions::new().open(name) {
                Ok(c) => return Ok(Box::new(c)),
                Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) => {
                    tokio::time::sleep(Duration::from_millis(50)).await
                }
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "the MCP pipe is busy",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Уникальный адрес на тест: на Unix — сокет во временном каталоге, на Windows — новое случайное имя pipe.
    fn temp_endpoint(dir: &Path) -> Endpoint {
        Endpoint::for_app(dir, "glafira-pg-test")
    }

    #[tokio::test]
    async fn connects_and_carries_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let ep = temp_endpoint(dir.path());
        let mut l = Listener::bind(&ep).await.unwrap();
        let srv = tokio::spawn(async move {
            let mut s = l.accept().await.unwrap();
            let mut buf = [0u8; 5];
            s.read_exact(&mut buf).await.unwrap();
            s.write_all(&buf).await.unwrap();
        });
        let mut c = connect(&ep).await.unwrap();
        c.write_all(b"hello").await.unwrap();
        let mut back = [0u8; 5];
        c.read_exact(&mut back).await.unwrap();
        assert_eq!(&back, b"hello");
        srv.await.unwrap();
    }

    #[test]
    fn endpoint_is_a_string_in_json() {
        let unix = Endpoint::Unix(PathBuf::from("/tmp/x/mcp.sock"));
        assert_eq!(serde_json::to_value(&unix).unwrap(), "/tmp/x/mcp.sock");
        assert_eq!(
            serde_json::from_value::<Endpoint>("/tmp/x/mcp.sock".into()).unwrap(),
            unix
        );
        let pipe = r"\\.\pipe\glafira-pg-mcp-0123456789abcdef";
        assert_eq!(Endpoint::parse(pipe), Endpoint::Pipe(pipe.to_string()));
        assert_eq!(Endpoint::parse(pipe).to_string(), pipe);
        assert_eq!(serde_json::to_value(Endpoint::parse(pipe)).unwrap(), pipe);
        assert_eq!(
            serde_json::from_value::<Endpoint>(pipe.into()).unwrap(),
            Endpoint::Pipe(pipe.to_string())
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn socket_is_private_stale_file_is_replaced_and_removed_on_drop() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let ep = temp_endpoint(dir.path());
        let Endpoint::Unix(path) = &ep else {
            panic!("на Unix адрес — сокет: {ep}")
        };
        std::fs::write(path, b"stale").unwrap();
        let l = Listener::bind(&ep).await.unwrap();
        let mode = std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let e = Listener::bind(&ep).await.err().unwrap();
        assert_eq!(e.kind(), io::ErrorKind::AddrInUse);
        drop(l);
        assert!(!path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn long_data_dir_falls_back_to_short_socket() {
        let long = Path::new("/").join("d".repeat(120));
        let ep = Endpoint::for_app(&long, "glafira-pg");
        let Endpoint::Unix(p) = &ep else {
            panic!("{ep}")
        };
        assert!(p.ends_with("glafira-pg-mcp.sock"), "{ep}");
        assert!(!p.starts_with(&long));
        assert_eq!(
            Endpoint::for_app(Path::new("/tmp/x"), "glafira-pg"),
            Endpoint::Unix(PathBuf::from("/tmp/x/mcp.sock"))
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn pipe_endpoint_is_refused_on_unix() {
        let ep = Endpoint::parse(r"\\.\pipe\glafira-pg-mcp-0123456789abcdef");
        let e = Listener::bind(&ep).await.err().unwrap();
        assert_eq!(e.kind(), io::ErrorKind::Unsupported);
        let e = connect(&ep).await.err().unwrap();
        assert_eq!(e.kind(), io::ErrorKind::Unsupported);
    }

    #[cfg(windows)]
    #[test]
    fn pipe_name_has_the_slug_and_a_fresh_suffix() {
        let dir = std::env::temp_dir();
        let a = Endpoint::for_app(&dir, "glafira-pg").to_string();
        let b = Endpoint::for_app(&dir, "glafira-pg").to_string();
        let prefix = r"\\.\pipe\glafira-pg-mcp-";
        assert!(a.starts_with(prefix) && a.len() == prefix.len() + 16, "{a}");
        assert!(
            a[prefix.len()..].chars().all(|c| c.is_ascii_hexdigit()),
            "{a}"
        );
        assert_ne!(a, b, "новое имя pipe на каждый запуск");
    }
}
