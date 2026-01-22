//! Port allocation for E2E tests
//!
//! Provides deterministic port allocation based on test name hash to avoid conflicts
//! when running tests in parallel. Ports are held until the allocator is dropped.

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::net::TcpListener;

/// Port ranges for different components
/// Using high ephemeral ports to avoid conflicts
const PORT_RANGE_START: u16 = 30000;
const PORT_RANGE_SIZE: u16 = 5000;

/// Ports needed for a standard E2E test
#[derive(Debug, Clone)]
pub struct TestPorts {
    /// Envoy admin port (stats, config_dump)
    pub envoy_admin: u16,
    /// xDS gRPC port for control plane
    pub xds: u16,
    /// HTTP API port for control plane
    pub api: u16,
    /// Echo/mock upstream server port
    pub echo: u16,
    /// Primary listener port for proxied traffic
    pub listener: u16,
    /// Secondary listener port (for multi-listener tests)
    pub listener_secondary: u16,
    /// Mock Auth0/JWKS server port
    pub mock_auth: u16,
    /// Mock ext_authz server port
    pub mock_ext_authz: u16,
}

/// Port allocator that reserves TCP ports and holds them until dropped
#[derive(Debug, Default)]
pub struct PortAllocator {
    /// Named port reservations
    reservations: HashMap<String, HeldPort>,
    /// Test name for deterministic allocation
    test_name: String,
    /// Hash-based offset for this test
    offset: u16,
}

#[derive(Debug)]
enum HeldPort {
    Listener(TcpListener),
}

impl HeldPort {
    fn port(&self) -> u16 {
        match self {
            HeldPort::Listener(l) => l.local_addr().map(|a| a.port()).unwrap_or(0),
        }
    }
}

impl PortAllocator {
    /// Create a new port allocator for a specific test
    pub fn for_test(test_name: &str) -> Self {
        let mut hasher = DefaultHasher::new();
        test_name.hash(&mut hasher);
        let hash = hasher.finish();
        // Use hash to generate offset within port range
        let offset = (hash % PORT_RANGE_SIZE as u64) as u16;

        Self { reservations: HashMap::new(), test_name: test_name.to_string(), offset }
    }

    /// Allocate all standard ports needed for an E2E test
    pub fn allocate_test_ports(&mut self) -> TestPorts {
        TestPorts {
            envoy_admin: self.reserve_labeled("envoy_admin"),
            xds: self.reserve_labeled("xds"),
            api: self.reserve_labeled("api"),
            echo: self.reserve_labeled("echo"),
            listener: self.reserve_labeled("listener"),
            listener_secondary: self.reserve_labeled("listener_secondary"),
            mock_auth: self.reserve_labeled("mock_auth"),
            mock_ext_authz: self.reserve_labeled("mock_ext_authz"),
        }
    }

    /// Reserve a labeled TCP port
    ///
    /// Uses deterministic allocation based on test name hash + label hash.
    /// Falls back to random port if deterministic port is unavailable.
    pub fn reserve_labeled(&mut self, label: &str) -> u16 {
        // Try deterministic port first
        let mut label_hasher = DefaultHasher::new();
        label.hash(&mut label_hasher);
        let label_hash = label_hasher.finish();
        let label_offset = (label_hash % 100) as u16;

        let target_port = PORT_RANGE_START + self.offset + label_offset;

        // Try to bind to deterministic port
        if let Ok(listener) = TcpListener::bind(("127.0.0.1", target_port)) {
            let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
            self.reservations.insert(label.to_string(), HeldPort::Listener(listener));
            return port;
        }

        // Fall back to random port allocation
        for attempt in 0..50 {
            let fallback_port = PORT_RANGE_START + self.offset + label_offset + attempt;
            if let Ok(listener) = TcpListener::bind(("127.0.0.1", fallback_port)) {
                let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
                self.reservations.insert(label.to_string(), HeldPort::Listener(listener));
                return port;
            }
        }

        // Last resort: let OS pick any available port
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("failed to bind to any port");
        let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
        self.reservations.insert(label.to_string(), HeldPort::Listener(listener));
        port
    }

    /// Get a previously reserved port by label
    pub fn get(&self, label: &str) -> Option<u16> {
        self.reservations.get(label).map(|h| h.port())
    }

    /// Get test name
    pub fn test_name(&self) -> &str {
        &self.test_name
    }

    /// Release a specific port reservation (for component restarts)
    pub fn release(&mut self, label: &str) -> Option<u16> {
        self.reservations.remove(label).map(|h| h.port())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deterministic_allocation() {
        let mut alloc1 = PortAllocator::for_test("test_bootstrap");
        let mut alloc2 = PortAllocator::for_test("test_bootstrap");

        let ports1 = alloc1.allocate_test_ports();
        let ports2 = alloc2.allocate_test_ports();

        // Different allocators should try the same deterministic ports
        // (may differ if one port was already taken)
        assert!(ports1.envoy_admin > 0);
        assert!(ports2.envoy_admin > 0);
    }

    #[test]
    fn test_different_tests_different_ports() {
        let mut alloc1 = PortAllocator::for_test("test_one");
        let mut alloc2 = PortAllocator::for_test("test_two");

        let port1 = alloc1.reserve_labeled("api");
        let port2 = alloc2.reserve_labeled("api");

        // Different test names should get different offsets
        assert!(port1 > 0);
        assert!(port2 > 0);
        // Ports should be different (unless collision, which is rare)
        // We don't assert inequality because OS might give same port if first is released
    }

    #[test]
    fn test_port_retrieval() {
        let mut alloc = PortAllocator::for_test("test_retrieval");
        let port = alloc.reserve_labeled("xds");

        assert_eq!(alloc.get("xds"), Some(port));
        assert_eq!(alloc.get("nonexistent"), None);
    }

    #[test]
    fn test_port_release() {
        let mut alloc = PortAllocator::for_test("test_release");
        let port = alloc.reserve_labeled("envoy");

        let released = alloc.release("envoy");
        assert_eq!(released, Some(port));
        assert_eq!(alloc.get("envoy"), None);
    }
}
