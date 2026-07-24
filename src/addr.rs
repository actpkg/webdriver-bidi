//! Loopback-only address validation.
//!
//! Duplicates the host's `wasi:sockets` ceiling on purpose (spec §3.1): the
//! ceiling is the enforcement boundary, this is defence in depth that fails
//! closed with a legible error.

use std::net::{IpAddr, SocketAddr};

#[derive(Debug)]
pub enum AddrError {
    NotAnIpLiteral(String),
    NotLoopback(IpAddr),
    InvalidPort,
}

impl std::fmt::Display for AddrError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AddrError::NotAnIpLiteral(h) => write!(
                f,
                "host {h:?} is not an IP literal; webdriver-bidi accepts only \
                 loopback literals such as 127.0.0.1 or ::1"
            ),
            AddrError::NotLoopback(ip) => {
                write!(
                    f,
                    "host {ip} is not a loopback address; scope is loopback-only"
                )
            }
            AddrError::InvalidPort => write!(f, "port must be non-zero"),
        }
    }
}

pub fn resolve(host: &str, port: u16) -> Result<SocketAddr, AddrError> {
    if port == 0 {
        return Err(AddrError::InvalidPort);
    }
    let ip: IpAddr = host
        .parse()
        .map_err(|_| AddrError::NotAnIpLiteral(host.to_string()))?;
    if !ip.is_loopback() {
        return Err(AddrError::NotLoopback(ip));
    }
    Ok(SocketAddr::new(ip, port))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_ipv4_loopback() {
        let a = resolve("127.0.0.1", 9222).unwrap();
        assert_eq!(a.to_string(), "127.0.0.1:9222");
    }

    #[test]
    fn accepts_ipv6_loopback() {
        let a = resolve("::1", 9222).unwrap();
        assert!(a.is_ipv6());
        assert_eq!(a.port(), 9222);
    }

    #[test]
    fn accepts_any_127_slash_8() {
        assert!(resolve("127.0.0.2", 9222).is_ok());
    }

    #[test]
    fn rejects_non_loopback_ip() {
        let e = resolve("10.0.0.5", 9222).unwrap_err();
        assert!(matches!(e, AddrError::NotLoopback(_)));
    }

    #[test]
    fn rejects_hostname() {
        // Not resolved on purpose: DNS rebinding + no lookup_host on wasip2.
        let e = resolve("localhost", 9222).unwrap_err();
        assert!(matches!(e, AddrError::NotAnIpLiteral(_)));
    }

    #[test]
    fn rejects_zero_port() {
        assert!(matches!(
            resolve("127.0.0.1", 0),
            Err(AddrError::InvalidPort)
        ));
    }
}
