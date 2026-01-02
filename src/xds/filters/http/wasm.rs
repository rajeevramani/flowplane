//! WebAssembly (WASM) HTTP filter configuration
//!
//! This module provides configuration types for the Envoy WASM filter,
//! which executes custom WebAssembly modules for request/response processing.

use crate::xds::filters::any_from_message;
use envoy_types::pb::envoy::config::core::v3::{
    async_data_source::Specifier as AsyncSpecifier, AsyncDataSource, DataSource,
};
use envoy_types::pb::envoy::extensions::filters::http::wasm::v3::Wasm as WasmProto;
use envoy_types::pb::envoy::extensions::wasm::v3::{
    plugin_config::Vm, FailurePolicy, PluginConfig, VmConfig as VmConfigProto,
};
use envoy_types::pb::google::protobuf::Any as EnvoyAny;
use prost::Message;
use prost_types::Struct as ProstStruct;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Type URL for WASM filter configuration
pub const WASM_TYPE_URL: &str = "type.googleapis.com/envoy.extensions.filters.http.wasm.v3.Wasm";

/// WASM filter configuration
///
/// This struct represents the plugin configuration that gets stored in the database.
/// The listener injection code extracts the inner config, so this struct directly
/// represents the plugin config without a wrapper.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct WasmConfig {
    /// Unique name for this plugin instance
    #[serde(default)]
    pub name: String,

    /// Root context ID for sharing state between plugin instances
    #[serde(default)]
    pub root_id: String,

    /// Virtual machine configuration
    pub vm_config: WasmVmConfig,

    /// Plugin-level configuration passed on each request (serialized as JSON)
    #[serde(default)]
    pub configuration: Option<serde_json::Value>,

    /// Behavior when VM fails
    #[serde(default)]
    pub failure_policy: Option<String>,
}

/// VM configuration
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct WasmVmConfig {
    /// VM instance identifier for sharing across plugins
    #[serde(default)]
    pub vm_id: String,

    /// WASM runtime to use (e.g., "envoy.wasm.runtime.wamr", "envoy.wasm.runtime.v8")
    #[serde(default = "default_runtime")]
    pub runtime: String,

    /// WASM binary source
    pub code: WasmCodeSource,

    /// VM-level configuration passed to the WASM module
    #[serde(default)]
    pub configuration: Option<serde_json::Value>,

    /// Allow loading precompiled WASM modules
    #[serde(default)]
    pub allow_precompiled: bool,

    /// NACK xDS update if WASM code is not in cache
    #[serde(default)]
    pub nack_on_code_cache_miss: bool,
}

fn default_runtime() -> String {
    "envoy.wasm.runtime.wamr".to_string()
}

/// WASM code source
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct WasmCodeSource {
    /// Local file source
    #[serde(default)]
    pub local: Option<WasmLocalSource>,

    /// Remote HTTP source
    #[serde(default)]
    pub remote: Option<WasmRemoteSource>,
}

/// Local WASM source
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct WasmLocalSource {
    /// Path to the WASM file
    #[serde(default)]
    pub filename: Option<String>,

    /// Base64-encoded WASM binary
    #[serde(default)]
    pub inline_bytes: Option<String>,

    /// WASM binary as string (for text format)
    #[serde(default)]
    pub inline_string: Option<String>,
}

/// Remote WASM source
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct WasmRemoteSource {
    /// HTTP URI configuration
    pub http_uri: WasmHttpUri,

    /// SHA256 hash for verification
    #[serde(default)]
    pub sha256: Option<String>,
}

/// HTTP URI for remote WASM fetch
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct WasmHttpUri {
    /// Full URI to fetch WASM binary
    pub uri: String,

    /// Cluster to use for fetching
    pub cluster: String,

    /// Fetch timeout (e.g., "30s")
    #[serde(default = "default_timeout")]
    pub timeout: String,
}

fn default_timeout() -> String {
    "30s".to_string()
}

impl WasmConfig {
    /// Convert to Envoy protobuf Any message
    pub fn to_any(&self) -> Result<EnvoyAny, crate::Error> {
        let proto = self.to_proto()?;
        Ok(any_from_message(WASM_TYPE_URL, &proto))
    }

    /// Convert to Envoy protobuf message
    fn to_proto(&self) -> Result<WasmProto, crate::Error> {
        let vm_config = self.vm_config.to_proto()?;

        // Convert plugin configuration to Any if provided
        let configuration = self.configuration.as_ref().map(|config| {
            let struct_value = json_to_prost_struct(config);
            EnvoyAny {
                type_url: "type.googleapis.com/google.protobuf.Struct".to_string(),
                value: struct_value.encode_to_vec(),
            }
        });

        // Parse failure policy
        let failure_policy = match self.failure_policy.as_deref() {
            Some("FAIL_OPEN") => FailurePolicy::FailOpen as i32,
            Some("FAIL_CLOSED") => FailurePolicy::FailClosed as i32,
            Some("FAIL_RELOAD") => FailurePolicy::FailReload as i32,
            _ => FailurePolicy::Unspecified as i32,
        };

        #[allow(deprecated)]
        let plugin_config = PluginConfig {
            name: self.name.clone(),
            root_id: self.root_id.clone(),
            vm: Some(Vm::VmConfig(vm_config)),
            configuration,
            fail_open: false, // Deprecated but required for struct initialization
            failure_policy,
            reload_config: None,
            capability_restriction_config: None,
            allow_on_headers_stop_iteration: None,
        };

        Ok(WasmProto { config: Some(plugin_config) })
    }
}

impl WasmVmConfig {
    fn to_proto(&self) -> Result<VmConfigProto, crate::Error> {
        let code = self.code.to_proto()?;

        // Convert VM configuration to Any if provided
        let configuration = self.configuration.as_ref().map(|config| {
            let struct_value = json_to_prost_struct(config);
            EnvoyAny {
                type_url: "type.googleapis.com/google.protobuf.Struct".to_string(),
                value: struct_value.encode_to_vec(),
            }
        });

        Ok(VmConfigProto {
            vm_id: self.vm_id.clone(),
            runtime: self.runtime.clone(),
            code: Some(code),
            configuration,
            allow_precompiled: self.allow_precompiled,
            nack_on_code_cache_miss: self.nack_on_code_cache_miss,
            environment_variables: None,
        })
    }
}

impl WasmCodeSource {
    fn to_proto(&self) -> Result<AsyncDataSource, crate::Error> {
        if let Some(local) = &self.local {
            let data_source = local.to_proto()?;
            return Ok(AsyncDataSource { specifier: Some(AsyncSpecifier::Local(data_source)) });
        }

        if let Some(remote) = &self.remote {
            return remote.to_proto();
        }

        Err(crate::Error::validation("WASM code source must specify either 'local' or 'remote'"))
    }
}

impl WasmLocalSource {
    fn to_proto(&self) -> Result<DataSource, crate::Error> {
        use envoy_types::pb::envoy::config::core::v3::data_source::Specifier;

        if let Some(filename) = &self.filename {
            return Ok(DataSource {
                specifier: Some(Specifier::Filename(filename.clone())),
                watched_directory: None,
            });
        }

        if let Some(inline_bytes) = &self.inline_bytes {
            // Decode base64
            let bytes =
                base64::Engine::decode(&base64::engine::general_purpose::STANDARD, inline_bytes)
                    .map_err(|e| {
                        crate::Error::validation(format!("Invalid base64 in inline_bytes: {}", e))
                    })?;
            return Ok(DataSource {
                specifier: Some(Specifier::InlineBytes(bytes)),
                watched_directory: None,
            });
        }

        if let Some(inline_string) = &self.inline_string {
            return Ok(DataSource {
                specifier: Some(Specifier::InlineString(inline_string.clone())),
                watched_directory: None,
            });
        }

        Err(crate::Error::validation(
            "Local WASM source must specify filename, inline_bytes, or inline_string",
        ))
    }
}

impl WasmRemoteSource {
    fn to_proto(&self) -> Result<AsyncDataSource, crate::Error> {
        use envoy_types::pb::envoy::config::core::v3::http_uri::HttpUpstreamType;
        use envoy_types::pb::envoy::config::core::v3::{HttpUri, RemoteDataSource};
        use envoy_types::pb::google::protobuf::Duration;

        // Parse timeout
        let timeout_secs = parse_duration_string(&self.http_uri.timeout)?;
        let timeout = Duration { seconds: timeout_secs as i64, nanos: 0 };

        let http_uri = HttpUri {
            uri: self.http_uri.uri.clone(),
            http_upstream_type: Some(HttpUpstreamType::Cluster(self.http_uri.cluster.clone())),
            timeout: Some(timeout),
        };

        let remote = RemoteDataSource {
            http_uri: Some(http_uri),
            sha256: self.sha256.clone().unwrap_or_default(),
            retry_policy: None,
        };

        Ok(AsyncDataSource { specifier: Some(AsyncSpecifier::Remote(remote)) })
    }
}

/// Parse duration string like "30s" to seconds
fn parse_duration_string(s: &str) -> Result<u64, crate::Error> {
    let s = s.trim();
    if let Some(stripped) = s.strip_suffix('s') {
        stripped
            .parse::<u64>()
            .map_err(|e| crate::Error::validation(format!("Invalid duration: {}", e)))
    } else if let Some(stripped) = s.strip_suffix('m') {
        stripped
            .parse::<u64>()
            .map(|m| m * 60)
            .map_err(|e| crate::Error::validation(format!("Invalid duration: {}", e)))
    } else {
        s.parse::<u64>().map_err(|e| crate::Error::validation(format!("Invalid duration: {}", e)))
    }
}

/// Convert serde_json::Value to prost_types::Struct
fn json_to_prost_struct(value: &serde_json::Value) -> ProstStruct {
    use prost_types::value::Kind;
    use prost_types::Value as ProstValue;

    fn convert_value(v: &serde_json::Value) -> ProstValue {
        let kind = match v {
            serde_json::Value::Null => Kind::NullValue(0),
            serde_json::Value::Bool(b) => Kind::BoolValue(*b),
            serde_json::Value::Number(n) => Kind::NumberValue(n.as_f64().unwrap_or(0.0)),
            serde_json::Value::String(s) => Kind::StringValue(s.clone()),
            serde_json::Value::Array(arr) => {
                let values: Vec<ProstValue> = arr.iter().map(convert_value).collect();
                Kind::ListValue(prost_types::ListValue { values })
            }
            serde_json::Value::Object(obj) => {
                let fields: std::collections::BTreeMap<String, ProstValue> =
                    obj.iter().map(|(k, v)| (k.clone(), convert_value(v))).collect();
                Kind::StructValue(ProstStruct { fields })
            }
        };
        ProstValue { kind: Some(kind) }
    }

    match value {
        serde_json::Value::Object(obj) => {
            let fields: std::collections::BTreeMap<String, ProstValue> =
                obj.iter().map(|(k, v)| (k.clone(), convert_value(v))).collect();
            ProstStruct { fields }
        }
        _ => ProstStruct { fields: std::collections::BTreeMap::new() },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wasm_config_to_any_local_file() {
        let config = WasmConfig {
            name: "my-filter".to_string(),
            root_id: "".to_string(),
            vm_config: WasmVmConfig {
                vm_id: "".to_string(),
                runtime: "envoy.wasm.runtime.wamr".to_string(),
                code: WasmCodeSource {
                    local: Some(WasmLocalSource {
                        filename: Some("/path/to/filter.wasm".to_string()),
                        inline_bytes: None,
                        inline_string: None,
                    }),
                    remote: None,
                },
                configuration: None,
                allow_precompiled: false,
                nack_on_code_cache_miss: false,
            },
            configuration: None,
            failure_policy: None,
        };

        let any = config.to_any().expect("should convert to Any");
        assert_eq!(any.type_url, WASM_TYPE_URL);
        assert!(!any.value.is_empty());
    }

    #[test]
    fn test_wasm_config_from_json() {
        // This matches the format stored in the database after inner config extraction
        let json = serde_json::json!({
            "name": "add_header",
            "vm_config": {
                "runtime": "envoy.wasm.runtime.v8",
                "code": {
                    "local": {
                        "filename": "/path/to/filter.wasm"
                    }
                }
            }
        });

        let config: WasmConfig = serde_json::from_value(json).expect("should parse");
        assert_eq!(config.name, "add_header");
        assert_eq!(config.vm_config.runtime, "envoy.wasm.runtime.v8");
    }

    #[test]
    fn test_parse_duration_string() {
        assert_eq!(parse_duration_string("30s").unwrap(), 30);
        assert_eq!(parse_duration_string("2m").unwrap(), 120);
        assert_eq!(parse_duration_string("60").unwrap(), 60);
    }
}
