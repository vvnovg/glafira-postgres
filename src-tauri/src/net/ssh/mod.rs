//! Туннель SSH: процесс `ssh -D` (OpenSSH), SOCKS5 к узлам через него, сироты после сбоя, супервизор.

pub mod orphans;
pub mod process;
pub mod socks;
pub mod tunnel;
