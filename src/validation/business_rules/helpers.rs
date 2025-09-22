use std::net::{Ipv4Addr, Ipv6Addr};

/// Check if a domain follows basic formatting rules (with optional leading wildcard).
pub(crate) fn is_valid_domain_format(domain: &str) -> bool {
    let domain_to_check = if domain.starts_with("*.") {
        &domain[2..]
    } else {
        domain
    };

    if domain_to_check.is_empty()
        || domain_to_check.starts_with('.')
        || domain_to_check.ends_with('.')
        || domain_to_check.contains("..")
    {
        return false;
    }

    for label in domain_to_check.split('.') {
        if label.is_empty() || label.len() > 63 {
            return false;
        }

        let chars: Vec<char> = label.chars().collect();
        if !chars[0].is_alphanumeric() || !chars[chars.len() - 1].is_alphanumeric() {
            return false;
        }

        if !label.chars().all(|c| c.is_alphanumeric() || c == '-') {
            return false;
        }
    }

    true
}

/// Check whether the provided address is a valid IPv4, IPv6, or hostname.
pub(crate) fn is_valid_address_format(address: &str) -> bool {
    if address.parse::<Ipv4Addr>().is_ok() {
        return true;
    }

    if address.parse::<Ipv6Addr>().is_ok() {
        return true;
    }

    if matches!(address, "localhost" | "0.0.0.0" | "::") {
        return true;
    }

    is_valid_domain_format(address)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domain_format_validation() {
        assert!(is_valid_domain_format("example.com"));
        assert!(is_valid_domain_format("sub.example.com"));
        assert!(is_valid_domain_format("api-v1.example.com"));
        assert!(is_valid_domain_format("localhost"));
        assert!(is_valid_domain_format("*.example.com"));

        assert!(!is_valid_domain_format(""));
        assert!(!is_valid_domain_format(".example.com"));
        assert!(!is_valid_domain_format("example.com."));
        assert!(!is_valid_domain_format("example..com"));
        assert!(!is_valid_domain_format("-example.com"));
        assert!(!is_valid_domain_format("example-.com"));

        let long_label = "a".repeat(64);
        assert!(!is_valid_domain_format(&format!("{}.com", long_label)));
    }

    #[test]
    fn address_format_validation() {
        assert!(is_valid_address_format("192.168.1.1"));
        assert!(is_valid_address_format("127.0.0.1"));
        assert!(is_valid_address_format("0.0.0.0"));
        assert!(is_valid_address_format("::1"));
        assert!(is_valid_address_format("2001:db8::1"));
        assert!(is_valid_address_format("localhost"));
        assert!(is_valid_address_format("example.com"));

        assert!(!is_valid_address_format(""));
        assert!(!is_valid_address_format("256.256.256.256"));
        assert!(!is_valid_address_format("invalid..address"));
    }
}
