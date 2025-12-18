//! Learning session filter injection for access logs and ExtProc.
//!
//! This module provides functions for injecting dynamic filters into listeners
//! based on active learning sessions:
//!
//! - **Access logs**: Injects HttpGrpcAccessLogConfig to capture request/response metadata
//! - **ExtProc**: Injects External Processing filter for body capture
//!
//! Both injections are temporary and only apply while learning sessions are active.

use crate::services::LearningSessionService;
use crate::xds::access_log::LearningSessionAccessLogConfig;
use crate::xds::filters::http::ext_proc::{ExtProcConfig, GrpcServiceConfig, ProcessingMode};
use crate::xds::helpers::ListenerModifier;
use crate::xds::resources::BuiltResource;
use crate::Result;
use envoy_types::pb::envoy::extensions::filters::network::http_connection_manager::v3::HttpFilter;
use tracing::{debug, info, warn};

/// Inject access log configuration into listeners for active learning sessions.
///
/// This function:
/// 1. Queries active learning sessions from the service
/// 2. For each active session, decodes the listener protobuf
/// 3. Injects HttpGrpcAccessLogConfig into the listener's filter chains
/// 4. Re-encodes the modified listener back into the BuiltResource
///
/// This enables dynamic access logging for routes in active learning sessions
/// without modifying the stored listener configuration.
///
/// # Arguments
///
/// * `built_listeners` - Mutable slice of built listener resources to modify
/// * `session_service` - Learning session service to query active sessions
/// * `grpc_address` - Address for the gRPC access log service (e.g., "127.0.0.1:18000")
pub async fn inject_access_logs(
    built_listeners: &mut [BuiltResource],
    session_service: &LearningSessionService,
    grpc_address: &str,
) -> Result<()> {
    // Query active learning sessions
    let active_sessions = session_service.list_active_sessions().await?;

    if active_sessions.is_empty() {
        debug!("No active learning sessions, skipping access log injection");
        return Ok(());
    }

    info!(
        session_count = active_sessions.len(),
        "Injecting access log configuration for active learning sessions"
    );

    // For each listener, check if it needs access log injection
    for built in built_listeners.iter_mut() {
        debug!(
            listener = %built.name,
            "Processing listener for access log injection"
        );

        let mut modifier = match ListenerModifier::decode(&built.resource.value, &built.name) {
            Ok(m) => m,
            Err(e) => {
                warn!(listener = %built.name, error = %e, "Failed to decode listener");
                continue;
            }
        };

        debug!(
            listener = %built.name,
            filter_chain_count = modifier.filter_chain_count(),
            "Decoded listener, checking filter chains"
        );

        // Check each active session to see if it applies to this listener
        for session in &active_sessions {
            debug!(
                listener = %built.name,
                session_id = %session.id,
                "Checking session for injection"
            );

            // Create access log config for this session
            let access_log_config = LearningSessionAccessLogConfig::new(
                session.id.clone(),
                session.team.clone(),
                grpc_address.to_string(),
            );

            let access_log = access_log_config.build_access_log()?;
            let session_id = session.id.clone();

            // Add access log using ListenerModifier
            let added = modifier.add_access_log(access_log, |name| name.contains(&session_id))?;

            if added > 0 {
                debug!(
                    listener = %built.name,
                    session_id = %session.id,
                    hcm_count = added,
                    "Injected access log configuration"
                );
            }
        }

        // If we modified the listener, update the built resource
        if let Some(encoded) = modifier.finish_if_modified() {
            built.resource.value = encoded;
            debug!(
                listener = %built.name,
                "Re-encoded listener with access log configuration"
            );
        }
    }

    Ok(())
}

/// Inject ExtProc filter configuration into listeners for active learning sessions.
///
/// This function:
/// 1. Queries active learning sessions from the service
/// 2. For each active session, decodes the listener protobuf
/// 3. Injects ExtProc HTTP filter into the listener's filter chains
/// 4. Re-encodes the modified listener back into the BuiltResource
///
/// This enables dynamic body capture for routes in active learning sessions
/// without modifying the stored listener configuration.
///
/// The ExtProc filter is configured to:
/// - Buffer request and response bodies up to 10KB
/// - Send bodies to the Flowplane ExtProc service
/// - Fail-open (requests continue even if ExtProc fails)
///
/// # Arguments
///
/// * `built_listeners` - Mutable slice of built listener resources to modify
/// * `session_service` - Learning session service to query active sessions
pub async fn inject_ext_proc(
    built_listeners: &mut [BuiltResource],
    session_service: &LearningSessionService,
) -> Result<()> {
    // Query active learning sessions
    let active_sessions = session_service.list_active_sessions().await?;

    if active_sessions.is_empty() {
        debug!("No active learning sessions, skipping ExtProc injection");
        return Ok(());
    }

    info!(
        session_count = active_sessions.len(),
        "Injecting ExtProc configuration for active learning sessions"
    );

    // For each listener, check if it needs ExtProc injection
    for built in built_listeners.iter_mut() {
        debug!(
            listener = %built.name,
            "Processing listener for ExtProc injection"
        );

        let mut modifier = match ListenerModifier::decode(&built.resource.value, &built.name) {
            Ok(m) => m,
            Err(e) => {
                warn!(listener = %built.name, error = %e, "Failed to decode listener");
                continue;
            }
        };

        // Check each active session to see if it applies to this listener
        for session in &active_sessions {
            debug!(
                listener = %built.name,
                session_id = %session.id,
                "Checking session for ExtProc injection"
            );

            // Create ExtProc config for body capture
            let ext_proc_config = ExtProcConfig {
                grpc_service: GrpcServiceConfig {
                    target_uri: "flowplane_ext_proc_service".to_string(),
                    timeout_seconds: 5,
                },
                failure_mode_allow: true, // Fail-open: requests continue even if ExtProc fails
                processing_mode: Some(ProcessingMode {
                    request_header_mode: Some("SEND".to_string()),
                    response_header_mode: Some("SEND".to_string()),
                    request_body_mode: Some("BUFFERED".to_string()), // Capture request body
                    response_body_mode: Some("BUFFERED".to_string()), // Capture response body
                    request_trailer_mode: Some("SKIP".to_string()),
                    response_trailer_mode: Some("SKIP".to_string()),
                }),
                message_timeout_ms: Some(5000), // 5 second timeout per message
                request_attributes: vec![],
                response_attributes: vec![],
            };

            let ext_proc_any = ext_proc_config.to_any()?;

            // Create HTTP filter for ExtProc
            let ext_proc_filter = HttpFilter {
                name: format!("envoy.filters.http.ext_proc.session_{}", session.id),
                config_type: Some(
                    envoy_types::pb::envoy::extensions::filters::network::http_connection_manager::v3::http_filter::ConfigType::TypedConfig(ext_proc_any)
                ),
                is_optional: true, // Make it optional so requests continue if filter fails
                disabled: false,
            };

            // Add ExtProc filter using ListenerModifier
            let added = modifier.add_filter_if_name_not_contains(ext_proc_filter, &session.id)?;

            if added > 0 {
                debug!(
                    listener = %built.name,
                    session_id = %session.id,
                    hcm_count = added,
                    "Injected ExtProc filter for body capture"
                );
            }
        }

        // If we modified the listener, update the built resource
        if let Some(encoded) = modifier.finish_if_modified() {
            built.resource.value = encoded;
            debug!(
                listener = %built.name,
                "Re-encoded listener with ExtProc configuration"
            );
        }
    }

    Ok(())
}
