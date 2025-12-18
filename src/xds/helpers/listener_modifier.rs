//! Helper for modifying Envoy Listener protobuf resources.
//!
//! Provides a safe, ergonomic API for navigating and modifying Envoy Listener
//! protobufs, eliminating duplicated protobuf navigation code across the codebase.

use crate::Result;
use envoy_types::pb::envoy::config::accesslog::v3::AccessLog;
use envoy_types::pb::envoy::config::listener::v3::filter::ConfigType;
use envoy_types::pb::envoy::config::listener::v3::Listener;
use envoy_types::pb::envoy::extensions::filters::network::http_connection_manager::v3::{
    HttpConnectionManager, HttpFilter,
};
use prost::Message;

/// Helper for modifying Envoy Listener protobuf resources.
///
/// Encapsulates the common pattern of:
/// 1. Decoding a Listener from protobuf bytes
/// 2. Navigating to HTTP connection managers in filter chains
/// 3. Modifying HCM configuration (filters, access logs, etc.)
/// 4. Re-encoding the modified Listener
///
/// # Example
///
/// ```rust,ignore
/// use flowplane::xds::helpers::ListenerModifier;
///
/// let mut modifier = ListenerModifier::decode(&bytes, "my-listener")?;
///
/// modifier.add_filter_before_router(my_filter, false)?;
///
/// if let Some(encoded) = modifier.finish_if_modified() {
///     built_resource.value = encoded;
/// }
/// ```
pub struct ListenerModifier {
    listener: Listener,
    name: String,
    modified: bool,
}

impl ListenerModifier {
    /// Decode a Listener from protobuf bytes.
    ///
    /// # Arguments
    ///
    /// * `bytes` - Protobuf-encoded Listener bytes
    /// * `name` - Listener name (used for error messages)
    ///
    /// # Errors
    ///
    /// Returns an error if the bytes cannot be decoded as a Listener.
    pub fn decode(bytes: &[u8], name: &str) -> Result<Self> {
        let listener = Listener::decode(bytes).map_err(|e| {
            crate::Error::internal(format!("Failed to decode listener '{}': {}", name, e))
        })?;

        Ok(Self { listener, name: name.to_string(), modified: false })
    }

    /// Get the listener name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Get the number of filter chains in the listener.
    pub fn filter_chain_count(&self) -> usize {
        self.listener.filter_chains.len()
    }

    /// Check if the listener was modified.
    pub fn is_modified(&self) -> bool {
        self.modified
    }

    /// Iterate over all HTTP connection managers and apply a modification function.
    ///
    /// The callback receives the HCM and filter chain index, and should return
    /// `Ok(true)` if it modified the HCM, `Ok(false)` if no changes were made.
    ///
    /// # Arguments
    ///
    /// * `f` - Callback function that receives a mutable HCM reference and filter chain index
    ///
    /// # Returns
    ///
    /// The number of HCMs that were modified.
    pub fn for_each_hcm<F>(&mut self, mut f: F) -> Result<usize>
    where
        F: FnMut(&mut HttpConnectionManager, usize) -> Result<bool>,
    {
        let mut modified_count = 0;

        for (fc_idx, filter_chain) in self.listener.filter_chains.iter_mut().enumerate() {
            for filter in filter_chain.filters.iter_mut() {
                if filter.name == "envoy.filters.network.http_connection_manager" {
                    if let Some(ConfigType::TypedConfig(typed_config)) = &mut filter.config_type {
                        let mut hcm = HttpConnectionManager::decode(&typed_config.value[..])
                            .map_err(|e| {
                                crate::Error::internal(format!(
                                    "Failed to decode HCM for listener '{}': {}",
                                    self.name, e
                                ))
                            })?;

                        if f(&mut hcm, fc_idx)? {
                            typed_config.value = hcm.encode_to_vec();
                            self.modified = true;
                            modified_count += 1;
                        }
                    }
                }
            }
        }

        Ok(modified_count)
    }

    /// Add an HTTP filter before the router filter in all HCMs.
    ///
    /// The router filter must be the last filter in the chain, so new filters
    /// are inserted just before it.
    ///
    /// # Arguments
    ///
    /// * `filter` - The HTTP filter to add
    /// * `skip_if_exists` - If true, skip adding if a filter with the same name already exists
    ///
    /// # Returns
    ///
    /// The number of HCMs where the filter was added.
    pub fn add_filter_before_router(
        &mut self,
        filter: HttpFilter,
        skip_if_exists: bool,
    ) -> Result<usize> {
        self.for_each_hcm(|hcm, _fc_idx| {
            // Check if filter already exists
            if skip_if_exists && hcm.http_filters.iter().any(|f| f.name == filter.name) {
                return Ok(false);
            }

            // Find router position
            let router_pos = hcm
                .http_filters
                .iter()
                .position(|f| f.name == "envoy.filters.http.router")
                .unwrap_or(hcm.http_filters.len());

            hcm.http_filters.insert(router_pos, filter.clone());
            Ok(true)
        })
    }

    /// Replace an existing filter or add if not present.
    ///
    /// If a filter with the same name exists, it is replaced.
    /// Otherwise, the filter is inserted before the router.
    ///
    /// # Arguments
    ///
    /// * `filter` - The HTTP filter to add or replace
    ///
    /// # Returns
    ///
    /// The number of HCMs where the filter was added or replaced.
    pub fn replace_or_add_filter(&mut self, filter: HttpFilter) -> Result<usize> {
        self.for_each_hcm(|hcm, _fc_idx| {
            // Check if filter already exists
            if let Some(idx) = hcm.http_filters.iter().position(|f| f.name == filter.name) {
                hcm.http_filters[idx] = filter.clone();
            } else {
                // Insert before router
                let router_pos = hcm
                    .http_filters
                    .iter()
                    .position(|f| f.name == "envoy.filters.http.router")
                    .unwrap_or(hcm.http_filters.len());

                hcm.http_filters.insert(router_pos, filter.clone());
            }
            Ok(true)
        })
    }

    /// Add an access log to all HCMs.
    ///
    /// # Arguments
    ///
    /// * `access_log` - The access log configuration to add
    /// * `duplicate_check` - Closure that returns true if the access log should be skipped
    ///   (receives the existing access log name)
    ///
    /// # Returns
    ///
    /// The number of HCMs where the access log was added.
    pub fn add_access_log<F>(&mut self, access_log: AccessLog, duplicate_check: F) -> Result<usize>
    where
        F: Fn(&str) -> bool,
    {
        self.for_each_hcm(|hcm, _fc_idx| {
            // Check for duplicates
            let already_exists = hcm.access_log.iter().any(|al| duplicate_check(&al.name));

            if already_exists {
                return Ok(false);
            }

            hcm.access_log.push(access_log.clone());
            Ok(true)
        })
    }

    /// Add an HTTP filter only if it doesn't already exist (checking by substring match).
    ///
    /// This is useful for session-specific filters where the filter name contains
    /// a session ID.
    ///
    /// # Arguments
    ///
    /// * `filter` - The HTTP filter to add
    /// * `name_contains` - Substring to check in existing filter names
    ///
    /// # Returns
    ///
    /// The number of HCMs where the filter was added.
    pub fn add_filter_if_name_not_contains(
        &mut self,
        filter: HttpFilter,
        name_contains: &str,
    ) -> Result<usize> {
        let name_contains = name_contains.to_string();

        self.for_each_hcm(move |hcm, _fc_idx| {
            // Check if a filter containing the substring already exists
            let already_exists = hcm.http_filters.iter().any(|f| f.name.contains(&name_contains));

            if already_exists {
                return Ok(false);
            }

            // Insert before router
            let router_pos = hcm
                .http_filters
                .iter()
                .position(|f| f.name == "envoy.filters.http.router")
                .unwrap_or(hcm.http_filters.len());

            hcm.http_filters.insert(router_pos, filter.clone());
            Ok(true)
        })
    }

    /// Get the route config names referenced by this listener's HCMs.
    ///
    /// Returns names from both RDS (route_config_name) and inline route configs.
    pub fn get_route_config_names(&self) -> Vec<String> {
        use envoy_types::pb::envoy::extensions::filters::network::http_connection_manager::v3::http_connection_manager::RouteSpecifier;

        let mut names = Vec::new();

        for filter_chain in &self.listener.filter_chains {
            for filter in &filter_chain.filters {
                if filter.name == "envoy.filters.network.http_connection_manager" {
                    if let Some(ConfigType::TypedConfig(typed_config)) = &filter.config_type {
                        if let Ok(hcm) = HttpConnectionManager::decode(&typed_config.value[..]) {
                            if let Some(route_specifier) = hcm.route_specifier {
                                match route_specifier {
                                    RouteSpecifier::Rds(rds) => {
                                        names.push(rds.route_config_name);
                                    }
                                    RouteSpecifier::RouteConfig(rc) => {
                                        names.push(rc.name);
                                    }
                                    RouteSpecifier::ScopedRoutes(_) => {
                                        // Scoped routes not yet supported
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        names
    }

    /// Finish modifications and return encoded bytes if the listener was modified.
    ///
    /// If no modifications were made, returns `None`.
    pub fn finish_if_modified(self) -> Option<Vec<u8>> {
        if self.modified {
            Some(self.listener.encode_to_vec())
        } else {
            None
        }
    }

    /// Finish modifications and always return encoded bytes.
    ///
    /// Use this when you need the encoded listener regardless of modifications.
    pub fn finish(self) -> Vec<u8> {
        self.listener.encode_to_vec()
    }

    /// Get a reference to the underlying Listener.
    ///
    /// Use with caution - direct modifications won't set the `modified` flag.
    pub fn listener(&self) -> &Listener {
        &self.listener
    }

    /// Mark the listener as modified.
    ///
    /// Use this if you made direct modifications via `listener()`.
    pub fn mark_modified(&mut self) {
        self.modified = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use envoy_types::pb::envoy::config::core::v3::{
        address::Address as AddressType, Address, SocketAddress,
    };
    use envoy_types::pb::envoy::config::listener::v3::{Filter, FilterChain};
    use envoy_types::pb::envoy::extensions::filters::network::http_connection_manager::v3::http_filter::ConfigType as HttpFilterConfigType;

    /// Create a minimal test listener with an HCM containing a router filter.
    fn create_test_listener(name: &str) -> Vec<u8> {
        use envoy_types::pb::google::protobuf::Any;

        let router_filter = HttpFilter {
            name: "envoy.filters.http.router".to_string(),
            config_type: None,
            is_optional: false,
            disabled: false,
        };

        let hcm = HttpConnectionManager {
            stat_prefix: "test".to_string(),
            http_filters: vec![router_filter],
            ..Default::default()
        };

        let hcm_any = Any {
            type_url: "type.googleapis.com/envoy.extensions.filters.network.http_connection_manager.v3.HttpConnectionManager".to_string(),
            value: hcm.encode_to_vec(),
        };

        let filter = Filter {
            name: "envoy.filters.network.http_connection_manager".to_string(),
            config_type: Some(ConfigType::TypedConfig(hcm_any)),
        };

        let filter_chain = FilterChain { filters: vec![filter], ..Default::default() };

        let listener = Listener {
            name: name.to_string(),
            filter_chains: vec![filter_chain],
            address: Some(Address {
                address: Some(AddressType::SocketAddress(SocketAddress {
                    address: "0.0.0.0".to_string(),
                    port_specifier: Some(
                        envoy_types::pb::envoy::config::core::v3::socket_address::PortSpecifier::PortValue(
                            8080,
                        ),
                    ),
                    ..Default::default()
                })),
            }),
            ..Default::default()
        };

        listener.encode_to_vec()
    }

    #[test]
    fn test_decode_listener() {
        let bytes = create_test_listener("test-listener");
        let modifier = ListenerModifier::decode(&bytes, "test-listener").unwrap();

        assert_eq!(modifier.name(), "test-listener");
        assert_eq!(modifier.filter_chain_count(), 1);
        assert!(!modifier.is_modified());
    }

    #[test]
    fn test_add_filter_before_router() {
        let bytes = create_test_listener("test-listener");
        let mut modifier = ListenerModifier::decode(&bytes, "test-listener").unwrap();

        let new_filter = HttpFilter {
            name: "envoy.filters.http.jwt_authn".to_string(),
            config_type: None,
            is_optional: false,
            disabled: false,
        };

        let added_count = modifier.add_filter_before_router(new_filter, false).unwrap();
        assert_eq!(added_count, 1);
        assert!(modifier.is_modified());

        // Verify the filter was added before router
        let encoded = modifier.finish_if_modified().unwrap();
        let listener = Listener::decode(&encoded[..]).unwrap();
        let filter_chain = &listener.filter_chains[0];
        let hcm_filter = &filter_chain.filters[0];

        if let Some(ConfigType::TypedConfig(typed_config)) = &hcm_filter.config_type {
            let hcm = HttpConnectionManager::decode(&typed_config.value[..]).unwrap();
            assert_eq!(hcm.http_filters.len(), 2);
            assert_eq!(hcm.http_filters[0].name, "envoy.filters.http.jwt_authn");
            assert_eq!(hcm.http_filters[1].name, "envoy.filters.http.router");
        } else {
            panic!("Expected TypedConfig");
        }
    }

    #[test]
    fn test_add_filter_skip_if_exists() {
        let bytes = create_test_listener("test-listener");
        let mut modifier = ListenerModifier::decode(&bytes, "test-listener").unwrap();

        let filter = HttpFilter {
            name: "envoy.filters.http.jwt_authn".to_string(),
            config_type: None,
            is_optional: false,
            disabled: false,
        };

        // Add filter first time
        let added1 = modifier.add_filter_before_router(filter.clone(), true).unwrap();
        assert_eq!(added1, 1);

        // Try to add again with skip_if_exists=true
        let added2 = modifier.add_filter_before_router(filter.clone(), true).unwrap();
        assert_eq!(added2, 0);

        // Verify only one filter was added
        let encoded = modifier.finish();
        let listener = Listener::decode(&encoded[..]).unwrap();
        if let Some(ConfigType::TypedConfig(typed_config)) =
            &listener.filter_chains[0].filters[0].config_type
        {
            let hcm = HttpConnectionManager::decode(&typed_config.value[..]).unwrap();
            assert_eq!(hcm.http_filters.len(), 2); // jwt_authn + router
        }
    }

    #[test]
    fn test_replace_or_add_filter() {
        let bytes = create_test_listener("test-listener");
        let mut modifier = ListenerModifier::decode(&bytes, "test-listener").unwrap();

        // Add a filter first
        let filter1 = HttpFilter {
            name: "envoy.filters.http.jwt_authn".to_string(),
            config_type: None,
            is_optional: false,
            disabled: false,
        };
        modifier.add_filter_before_router(filter1, false).unwrap();

        // Replace it with updated config
        let filter2 = HttpFilter {
            name: "envoy.filters.http.jwt_authn".to_string(),
            config_type: Some(HttpFilterConfigType::TypedConfig(
                envoy_types::pb::google::protobuf::Any {
                    type_url: "test.type".to_string(),
                    value: vec![1, 2, 3],
                },
            )),
            is_optional: true,
            disabled: false,
        };
        modifier.replace_or_add_filter(filter2).unwrap();

        // Verify replacement happened
        let encoded = modifier.finish();
        let listener = Listener::decode(&encoded[..]).unwrap();
        if let Some(ConfigType::TypedConfig(typed_config)) =
            &listener.filter_chains[0].filters[0].config_type
        {
            let hcm = HttpConnectionManager::decode(&typed_config.value[..]).unwrap();
            assert_eq!(hcm.http_filters.len(), 2); // Still jwt_authn + router
            assert!(hcm.http_filters[0].is_optional); // Updated field
        }
    }

    #[test]
    fn test_add_access_log() {
        let bytes = create_test_listener("test-listener");
        let mut modifier = ListenerModifier::decode(&bytes, "test-listener").unwrap();

        let access_log = AccessLog {
            name: "session_123_access_log".to_string(),
            filter: None,
            config_type: None,
        };

        // Add access log
        let added = modifier
            .add_access_log(access_log.clone(), |name| name.contains("session_123"))
            .unwrap();
        assert_eq!(added, 1);

        // Try to add again - should be skipped
        let added2 =
            modifier.add_access_log(access_log, |name| name.contains("session_123")).unwrap();
        assert_eq!(added2, 0);
    }

    #[test]
    fn test_add_filter_if_name_not_contains() {
        let bytes = create_test_listener("test-listener");
        let mut modifier = ListenerModifier::decode(&bytes, "test-listener").unwrap();

        let filter = HttpFilter {
            name: "envoy.filters.http.ext_proc.session_abc123".to_string(),
            config_type: None,
            is_optional: true,
            disabled: false,
        };

        // Add filter first time
        let added1 =
            modifier.add_filter_if_name_not_contains(filter.clone(), "session_abc123").unwrap();
        assert_eq!(added1, 1);

        // Try to add again - should be skipped
        let added2 = modifier.add_filter_if_name_not_contains(filter, "session_abc123").unwrap();
        assert_eq!(added2, 0);
    }

    #[test]
    fn test_finish_if_modified_returns_none_when_unmodified() {
        let bytes = create_test_listener("test-listener");
        let modifier = ListenerModifier::decode(&bytes, "test-listener").unwrap();

        assert!(modifier.finish_if_modified().is_none());
    }

    #[test]
    fn test_for_each_hcm_callback() {
        let bytes = create_test_listener("test-listener");
        let mut modifier = ListenerModifier::decode(&bytes, "test-listener").unwrap();

        let mut callback_count = 0;
        modifier
            .for_each_hcm(|_hcm, fc_idx| {
                callback_count += 1;
                assert_eq!(fc_idx, 0);
                Ok(false)
            })
            .unwrap();

        assert_eq!(callback_count, 1);
        assert!(!modifier.is_modified());
    }

    #[test]
    fn test_decode_invalid_bytes() {
        let result = ListenerModifier::decode(&[1, 2, 3], "invalid");
        assert!(result.is_err());
    }
}
