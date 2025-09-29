//! Request validation helpers for the Platform API abstraction.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::platform_api::{
    filter_overrides::{canonicalize_filter_overrides, validate_filter_overrides},
    materializer::{ApiDefinitionSpec, RouteSpec},
};
use crate::validation::validate_host;
use crate::Error;
use utoipa::ToSchema;

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateApiDefinitionBody {
    pub team: String,
    pub domain: String,
    #[serde(default)]
    pub listener_isolation: bool,
    #[serde(default)]
    pub listener: Option<IsolationListenerBody>,
    #[serde(default)]
    pub tls: Option<Value>,
    pub routes: Vec<RouteBody>,
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct IsolationListenerBody {
    #[serde(default)]
    #[schema(example = "payments-shared-listener")]
    pub name: Option<String>,
    #[schema(example = "0.0.0.0")]
    pub bind_address: String,
    #[schema(example = 10010, minimum = 1, maximum = 65535)]
    pub port: u16,
    #[serde(default)]
    #[schema(example = "HTTP")]
    pub protocol: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AppendRouteBody {
    pub route: RouteBody,
    #[serde(default)]
    pub deployment_note: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RouteBody {
    #[serde(rename = "match")]
    pub matcher: RouteMatchBody,
    pub cluster: RouteClusterBody,
    #[serde(default)]
    pub timeout_seconds: Option<i64>,
    #[serde(default)]
    pub rewrite: Option<RouteRewriteBody>,
    #[serde(default)]
    pub filters: Option<Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
pub struct RouteMatchBody {
    #[serde(default)]
    pub prefix: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RouteClusterBody {
    pub name: String,
    pub endpoint: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RouteRewriteBody {
    #[serde(default)]
    pub prefix: Option<String>,
    #[serde(default)]
    pub regex: Option<String>,
    #[serde(default)]
    pub substitution: Option<String>,
}

impl CreateApiDefinitionBody {
    fn validate_payload(&self) -> Result<(), Error> {
        ensure_non_empty(&self.team, "team", 100)?;
        ensure_non_empty(&self.domain, "domain", 253)?;
        validate_host(&self.domain).map_err(|_| {
            Error::validation("domain must contain alphanumeric, '.' or '-' characters")
        })?;

        if self.routes.is_empty() {
            return Err(Error::validation("at least one route is required"));
        }

        if self.routes.len() > 50 {
            return Err(Error::validation("no more than 50 routes can be provided"));
        }

        for route in &self.routes {
            route.validate_payload()?;
        }

        // Isolation mode requires explicit listener fields
        if self.listener_isolation {
            let listener = self.listener.as_ref().ok_or_else(|| {
                Error::validation("listener is required when listenerIsolation is true")
            })?;
            listener.validate()?;
        }

        Ok(())
    }

    pub fn into_spec(self) -> Result<ApiDefinitionSpec, Error> {
        self.validate_payload()?;

        let CreateApiDefinitionBody { team, domain, listener_isolation, listener, tls, routes } =
            self;

        let mut specs = Vec::with_capacity(routes.len());
        for (idx, route) in routes.into_iter().enumerate() {
            specs.push(route.into_route_spec(Some(idx as i64), None)?);
        }

        let listener_spec = listener.map(|l| l.into_spec());

        Ok(ApiDefinitionSpec {
            team,
            domain,
            listener_isolation,
            isolation_listener: listener_spec,
            tls_config: tls,
            routes: specs,
        })
    }
}

impl IsolationListenerBody {
    fn validate(&self) -> Result<(), Error> {
        ensure_non_empty(&self.bind_address, "listener.bindAddress", 255)?;
        if let Some(proto) = &self.protocol {
            let p = proto.to_uppercase();
            if p != "HTTP" && p != "HTTPS" {
                return Err(Error::validation(
                    "listener.protocol must be either 'HTTP' or 'HTTPS'",
                ));
            }
        }
        if self.port == 0 {
            return Err(Error::validation("listener.port must be greater than zero"));
        }
        Ok(())
    }

    fn into_spec(self) -> crate::platform_api::materializer::ListenerInput {
        crate::platform_api::materializer::ListenerInput {
            name: self.name,
            bind_address: self.bind_address,
            port: self.port as u32,
            protocol: self.protocol.unwrap_or_else(|| "HTTP".to_string()),
            tls_config: None,
        }
    }
}

impl AppendRouteBody {
    fn validate_payload(&self) -> Result<(), Error> {
        self.route.validate_payload()?;
        if let Some(note) = &self.deployment_note {
            if note.len() > 500 {
                return Err(Error::validation("deploymentNote cannot exceed 500 characters"));
            }
        }
        Ok(())
    }

    pub fn into_route_spec(self, desired_order: Option<usize>) -> Result<RouteSpec, Error> {
        self.validate_payload()?;
        let AppendRouteBody { route, deployment_note } = self;
        let order = desired_order.map(|idx| idx as i64);
        route.into_route_spec(order, deployment_note)
    }
}

impl RouteBody {
    fn validate_payload(&self) -> Result<(), Error> {
        self.matcher.validate()?;
        self.cluster.validate()?;

        if let Some(timeout) = self.timeout_seconds {
            if timeout <= 0 {
                return Err(Error::validation("timeoutSeconds must be greater than zero"));
            }
            if timeout > 3600 {
                return Err(Error::validation("timeoutSeconds cannot exceed 3600"));
            }
        }

        if let Some(rewrite) = &self.rewrite {
            rewrite.validate()?;
        }

        validate_filter_overrides(&self.filters)?;

        Ok(())
    }

    fn into_route_spec(
        self,
        order: Option<i64>,
        deployment_note: Option<String>,
    ) -> Result<RouteSpec, Error> {
        let RouteBody { matcher, cluster, timeout_seconds, rewrite, filters } = self;

        let (match_type, match_value) = matcher.into_matcher()?;
        let (rewrite_prefix, rewrite_regex, rewrite_substitution) = match rewrite {
            Some(rewrite) => rewrite.into_parts(),
            None => (None, None, None),
        };

        Ok(RouteSpec {
            match_type,
            match_value,
            case_sensitive: true,
            rewrite_prefix,
            rewrite_regex,
            rewrite_substitution,
            upstream_targets: cluster.into_upstream_targets(),
            timeout_seconds,
            override_config: canonicalize_filter_overrides(filters)?,
            deployment_note,
            route_order: order,
        })
    }
}

impl RouteMatchBody {
    fn validate(&self) -> Result<(), Error> {
        if let Some(prefix) = &self.prefix {
            ensure_path(prefix, "match.prefix")?;
        }
        if let Some(path) = &self.path {
            ensure_path(path, "match.path")?;
        }
        if self.prefix.is_none() && self.path.is_none() {
            return Err(Error::validation("route match must include either prefix or path"));
        }
        Ok(())
    }

    fn into_matcher(self) -> Result<(String, String), Error> {
        if let Some(prefix) = self.prefix {
            return Ok(("prefix".to_string(), prefix));
        }
        if let Some(path) = self.path {
            return Ok(("path".to_string(), path));
        }
        Err(Error::validation("route match must include either prefix or path"))
    }
}

impl RouteClusterBody {
    fn validate(&self) -> Result<(), Error> {
        ensure_non_empty(&self.name, "cluster.name", 100)?;
        ensure_non_empty(&self.endpoint, "cluster.endpoint", 255)?;
        parse_endpoint(&self.endpoint)?;
        Ok(())
    }

    fn into_upstream_targets(self) -> Value {
        json!({
            "targets": [
                {
                    "name": self.name,
                    "endpoint": self.endpoint,
                }
            ]
        })
    }
}

impl RouteRewriteBody {
    fn validate(&self) -> Result<(), Error> {
        if let Some(prefix) = &self.prefix {
            ensure_path(prefix, "rewrite.prefix")?;
        }
        if let Some(regex) = &self.regex {
            if regex.is_empty() {
                return Err(Error::validation("rewrite.regex cannot be empty"));
            }
        }
        if let Some(substitution) = &self.substitution {
            if substitution.is_empty() {
                return Err(Error::validation("rewrite.substitution cannot be empty"));
            }
        }
        if self.regex.is_some() ^ self.substitution.is_some() {
            return Err(Error::validation(
                "rewrite.regex and rewrite.substitution must be provided together",
            ));
        }
        Ok(())
    }

    fn into_parts(self) -> (Option<String>, Option<String>, Option<String>) {
        (self.prefix, self.regex, self.substitution)
    }
}

fn ensure_non_empty(value: &str, field: &str, max_len: usize) -> Result<(), Error> {
    if value.trim().is_empty() {
        return Err(Error::validation(format!("{} cannot be empty", field)));
    }
    if value.len() > max_len {
        return Err(Error::validation(format!("{} cannot exceed {} characters", field, max_len)));
    }
    Ok(())
}

fn ensure_path(value: &str, field: &str) -> Result<(), Error> {
    if !value.starts_with('/') {
        return Err(Error::validation(format!("{} must start with '/'", field)));
    }
    if value.contains("//") {
        return Err(Error::validation(format!(
            "{} cannot contain consecutive '/' characters",
            field
        )));
    }
    Ok(())
}

fn parse_endpoint(endpoint: &str) -> Result<(), Error> {
    let (host, port_str) = endpoint
        .rsplit_once(':')
        .ok_or_else(|| Error::validation("cluster.endpoint must be in 'host:port' format"))?;

    validate_host(host)
        .map_err(|_| Error::validation("cluster.endpoint host contains invalid characters"))?;

    let port: u16 = port_str
        .parse()
        .map_err(|_| Error::validation("cluster.endpoint port must be a valid number"))?;
    if port == 0 {
        return Err(Error::validation("cluster.endpoint port must be greater than zero"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_payload_validation_passes() {
        let body = CreateApiDefinitionBody {
            team: "payments".to_string(),
            domain: "payments.flowplane.dev".to_string(),
            listener_isolation: false,
            listener: None,
            tls: None,
            routes: vec![RouteBody {
                matcher: RouteMatchBody { prefix: Some("/v1".to_string()), path: None },
                cluster: RouteClusterBody {
                    name: "backend".to_string(),
                    endpoint: "backend.svc.cluster.local:8443".to_string(),
                },
                timeout_seconds: Some(30),
                rewrite: None,
                filters: None,
            }],
        };

        assert!(body.into_spec().is_ok());
    }

    #[test]
    fn invalid_endpoint_fails_validation() {
        let route = RouteBody {
            matcher: RouteMatchBody { prefix: Some("/v1".to_string()), path: None },
            cluster: RouteClusterBody {
                name: "backend".to_string(),
                endpoint: "localhost".to_string(),
            },
            timeout_seconds: None,
            rewrite: None,
            filters: None,
        };

        assert!(route.validate_payload().is_err());
    }

    #[test]
    fn route_to_spec_conversion() {
        let route = RouteBody {
            matcher: RouteMatchBody { prefix: Some("/v1".to_string()), path: None },
            cluster: RouteClusterBody {
                name: "backend".to_string(),
                endpoint: "backend:8080".to_string(),
            },
            timeout_seconds: Some(15),
            rewrite: Some(RouteRewriteBody {
                prefix: Some("/internal".to_string()),
                regex: None,
                substitution: None,
            }),
            filters: Some(json!({ "cors": "allow-authenticated" })),
        };

        let spec = route.into_route_spec(Some(0), Some("deploy".to_string())).expect("route spec");
        assert_eq!(spec.match_type, "prefix");
        assert_eq!(spec.match_value, "/v1");
        assert_eq!(spec.route_order, Some(0));
        assert_eq!(spec.deployment_note.as_deref(), Some("deploy"));
    }
}
