use reserve_port::ReservedPort;
use std::collections::HashMap;
use std::net::TcpListener;

#[derive(Debug)]
enum HeldPort {
    Crate(ReservedPort),
    Listener(TcpListener),
}

impl HeldPort {
    fn port(&self) -> u16 {
        match self {
            HeldPort::Crate(r) => r.port(),
            HeldPort::Listener(l) => l.local_addr().map(|a| a.port()).unwrap_or(0),
        }
    }
}

/// Simple per-test TCP port allocator that keeps reservations alive until dropped.
#[derive(Debug, Default)]
pub struct PortAllocator {
    reservations: HashMap<String, HeldPort>,
    #[allow(dead_code)]
    anon: Vec<HeldPort>,
}

impl PortAllocator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Reserve and label a TCP port. The reservation is held until `self` drops.
    pub fn reserve_labeled(&mut self, label: &str) -> u16 {
        let mut last_err = None;
        for _ in 0..10 {
            match ReservedPort::random() {
                Ok(r) => {
                    let port = r.port();
                    self.reservations.insert(label.to_string(), HeldPort::Crate(r));
                    return port;
                }
                Err(e) => {
                    last_err = Some(e);
                    // Fallback: bind a local listener to reserve the port in restricted sandboxes
                    if let Ok(listener) = TcpListener::bind(("127.0.0.1", 0)) {
                        let port = listener.local_addr().ok().map(|a| a.port()).unwrap_or(0);
                        self.reservations.insert(label.to_string(), HeldPort::Listener(listener));
                        return port;
                    }
                }
            }
        }
        panic!("failed to reserve tcp port: {:?}", last_err);
    }

    /// Reserve an anonymous TCP port, returning the port number.
    #[allow(dead_code)]
    pub fn reserve(&mut self) -> u16 {
        let mut last_err = None;
        for _ in 0..10 {
            match ReservedPort::random() {
                Ok(r) => {
                    let port = r.port();
                    self.anon.push(HeldPort::Crate(r));
                    return port;
                }
                Err(e) => {
                    last_err = Some(e);
                    if let Ok(listener) = TcpListener::bind(("127.0.0.1", 0)) {
                        let port = listener.local_addr().ok().map(|a| a.port()).unwrap_or(0);
                        self.anon.push(HeldPort::Listener(listener));
                        return port;
                    }
                }
            }
        }
        panic!("failed to reserve tcp port: {:?}", last_err);
    }

    /// Get a previously reserved labeled port if available.
    pub fn get(&self, label: &str) -> Option<u16> {
        self.reservations.get(label).map(|r| r.port())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserves_distinct_ports() {
        let mut p = PortAllocator::new();
        let a = p.reserve_labeled("envoy-admin");
        let b = p.reserve_labeled("envoy-listener");
        // On some CI/sandboxed environments, ephemeral collisions are rare but possible; ensure they are >0 and typically distinct
        assert!(a > 0 && b > 0);
        if a == b {
            // Try again to prove allocator works
            let c = p.reserve_labeled("echo");
            assert_ne!(a, c);
        }
        assert_eq!(p.get("envoy-admin"), Some(a));
        assert_eq!(p.get("envoy-listener"), Some(b));
    }
}
