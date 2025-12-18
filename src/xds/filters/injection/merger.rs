//! JWT configuration merger for aggregating multiple JWT auth configs.
//!
//! When multiple filters attach JWT authentication to a listener,
//! their configurations need to be merged into a single JwtAuthentication
//! filter with all providers and requirements combined.

use crate::xds::filters::http::jwt_auth::{
    JwtAuthenticationConfig, JwtProviderConfig, JwtRequirementConfig, JwtRequirementRuleConfig,
};
use std::collections::HashMap;

/// Merges multiple JWT authentication configurations into a single config.
///
/// Envoy's `envoy.filters.http.jwt_authn` filter supports multiple providers,
/// but only one filter instance per HCM. This merger combines configurations
/// from multiple filter definitions into a single merged configuration.
///
/// # Merging Strategy
///
/// - **Providers**: All providers are combined. Duplicate provider names result
///   in the later configuration overwriting the earlier one.
/// - **Rules**: All rules are combined in order.
/// - **Requirement Map**: All entries are combined. Duplicate keys result in
///   overwrites.
/// - **Boolean flags**: OR semantics (true if any source is true)
/// - **Stat prefix**: Last non-empty value wins
///
/// # Example
///
/// ```rust,ignore
/// let mut merger = JwtConfigMerger::new();
///
/// merger.add(&jwt_config_1);
/// merger.add(&jwt_config_2);
///
/// let merged = merger.finish();
/// ```
#[derive(Debug, Default)]
pub struct JwtConfigMerger {
    providers: HashMap<String, JwtProviderConfig>,
    rules: Vec<JwtRequirementRuleConfig>,
    requirement_map: HashMap<String, JwtRequirementConfig>,
    bypass_cors_preflight: Option<bool>,
    strip_failure_response: Option<bool>,
    stat_prefix: Option<String>,
}

impl JwtConfigMerger {
    /// Create a new empty merger.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a JWT authentication configuration to the merger.
    ///
    /// The configuration's providers, rules, and requirement map entries
    /// are all added to the merged result.
    pub fn add(&mut self, config: &JwtAuthenticationConfig) {
        // Merge providers (later overwrites earlier on conflict)
        for (name, provider) in &config.providers {
            if let Some(existing) = self.providers.get(name) {
                tracing::warn!(
                    provider_name = %name,
                    old_issuer = ?existing.issuer,
                    new_issuer = ?provider.issuer,
                    "JWT provider name conflict - later config overwrites earlier"
                );
            }
            self.providers.insert(name.clone(), provider.clone());
        }

        // Append rules
        self.rules.extend(config.rules.clone());

        // Merge requirement map (later overwrites earlier on conflict)
        for (name, requirement) in &config.requirement_map {
            if self.requirement_map.contains_key(name) {
                tracing::warn!(
                    requirement_name = %name,
                    "JWT requirement map conflict - later config overwrites earlier"
                );
            }
            self.requirement_map.insert(name.clone(), requirement.clone());
        }

        // OR boolean flags
        if config.bypass_cors_preflight.unwrap_or(false) {
            self.bypass_cors_preflight = Some(true);
        }
        if config.strip_failure_response.unwrap_or(false) {
            self.strip_failure_response = Some(true);
        }

        // Last non-empty stat prefix wins
        if let Some(prefix) = &config.stat_prefix {
            if !prefix.is_empty() {
                self.stat_prefix = Some(prefix.clone());
            }
        }
    }

    /// Check if the merger has any providers.
    pub fn has_providers(&self) -> bool {
        !self.providers.is_empty()
    }

    /// Get the number of providers in the merged config.
    pub fn provider_count(&self) -> usize {
        self.providers.len()
    }

    /// Consume the merger and produce the final merged configuration.
    ///
    /// If the requirement_map is empty but providers exist, this will
    /// auto-populate the requirement_map with entries for each provider.
    pub fn finish(mut self) -> JwtAuthenticationConfig {
        // Auto-populate requirement_map if empty but providers exist
        if self.requirement_map.is_empty() && !self.providers.is_empty() {
            for provider_name in self.providers.keys() {
                self.requirement_map.insert(
                    provider_name.clone(),
                    JwtRequirementConfig::ProviderName { provider_name: provider_name.clone() },
                );
            }
        }

        JwtAuthenticationConfig {
            providers: self.providers,
            rules: self.rules,
            requirement_map: self.requirement_map,
            filter_state_rules: None, // Not merged (complex structure)
            bypass_cors_preflight: self.bypass_cors_preflight,
            strip_failure_response: self.strip_failure_response,
            stat_prefix: self.stat_prefix,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xds::filters::http::jwt_auth::{JwtJwksSourceConfig, LocalJwksConfig};

    fn create_jwt_config(provider_name: &str, issuer: &str) -> JwtAuthenticationConfig {
        let mut providers = HashMap::new();
        providers.insert(
            provider_name.to_string(),
            JwtProviderConfig {
                issuer: Some(issuer.to_string()),
                jwks: JwtJwksSourceConfig::Local(LocalJwksConfig {
                    inline_string: Some(r#"{"keys":[]}"#.to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            },
        );

        JwtAuthenticationConfig { providers, ..Default::default() }
    }

    #[test]
    fn test_merge_single_config() {
        let mut merger = JwtConfigMerger::new();
        let config = create_jwt_config("provider1", "https://issuer1.example.com");

        merger.add(&config);

        assert!(merger.has_providers());
        assert_eq!(merger.provider_count(), 1);

        let merged = merger.finish();
        assert_eq!(merged.providers.len(), 1);
        assert!(merged.providers.contains_key("provider1"));
    }

    #[test]
    fn test_merge_multiple_configs() {
        let mut merger = JwtConfigMerger::new();

        merger.add(&create_jwt_config("provider1", "https://issuer1.example.com"));
        merger.add(&create_jwt_config("provider2", "https://issuer2.example.com"));

        let merged = merger.finish();

        assert_eq!(merged.providers.len(), 2);
        assert!(merged.providers.contains_key("provider1"));
        assert!(merged.providers.contains_key("provider2"));
    }

    #[test]
    fn test_duplicate_provider_overwrites() {
        let mut merger = JwtConfigMerger::new();

        merger.add(&create_jwt_config("provider1", "https://old-issuer.example.com"));
        merger.add(&create_jwt_config("provider1", "https://new-issuer.example.com"));

        let merged = merger.finish();

        assert_eq!(merged.providers.len(), 1);
        assert_eq!(
            merged.providers.get("provider1").unwrap().issuer,
            Some("https://new-issuer.example.com".to_string())
        );
    }

    #[test]
    fn test_boolean_flags_or_semantics() {
        let mut merger = JwtConfigMerger::new();

        let mut config1 = create_jwt_config("provider1", "https://issuer1.example.com");
        config1.bypass_cors_preflight = Some(false);
        config1.strip_failure_response = Some(true);

        let mut config2 = create_jwt_config("provider2", "https://issuer2.example.com");
        config2.bypass_cors_preflight = Some(true);
        config2.strip_failure_response = Some(false);

        merger.add(&config1);
        merger.add(&config2);

        let merged = merger.finish();

        // Both should be true (OR semantics)
        assert_eq!(merged.bypass_cors_preflight, Some(true));
        assert_eq!(merged.strip_failure_response, Some(true));
    }

    #[test]
    fn test_auto_populate_requirement_map() {
        let mut merger = JwtConfigMerger::new();

        merger.add(&create_jwt_config("provider1", "https://issuer1.example.com"));
        merger.add(&create_jwt_config("provider2", "https://issuer2.example.com"));

        let merged = merger.finish();

        // Should auto-populate requirement_map
        assert_eq!(merged.requirement_map.len(), 2);
        assert!(merged.requirement_map.contains_key("provider1"));
        assert!(merged.requirement_map.contains_key("provider2"));
    }

    #[test]
    fn test_rules_are_combined() {
        let mut merger = JwtConfigMerger::new();

        let mut config1 = create_jwt_config("provider1", "https://issuer1.example.com");
        config1.rules.push(JwtRequirementRuleConfig {
            r#match: None,
            requires: Some(JwtRequirementConfig::ProviderName {
                provider_name: "provider1".to_string(),
            }),
            requirement_name: None,
        });

        let mut config2 = create_jwt_config("provider2", "https://issuer2.example.com");
        config2.rules.push(JwtRequirementRuleConfig {
            r#match: None,
            requires: Some(JwtRequirementConfig::ProviderName {
                provider_name: "provider2".to_string(),
            }),
            requirement_name: None,
        });

        merger.add(&config1);
        merger.add(&config2);

        let merged = merger.finish();

        assert_eq!(merged.rules.len(), 2);
    }

    #[test]
    fn test_stat_prefix_last_wins() {
        let mut merger = JwtConfigMerger::new();

        let mut config1 = create_jwt_config("provider1", "https://issuer1.example.com");
        config1.stat_prefix = Some("first_prefix".to_string());

        let mut config2 = create_jwt_config("provider2", "https://issuer2.example.com");
        config2.stat_prefix = Some("second_prefix".to_string());

        merger.add(&config1);
        merger.add(&config2);

        let merged = merger.finish();

        assert_eq!(merged.stat_prefix, Some("second_prefix".to_string()));
    }

    #[test]
    fn test_empty_merger() {
        let merger = JwtConfigMerger::new();
        assert!(!merger.has_providers());
        assert_eq!(merger.provider_count(), 0);

        let merged = merger.finish();
        assert!(merged.providers.is_empty());
        assert!(merged.rules.is_empty());
        assert!(merged.requirement_map.is_empty());
    }
}
