//! Current-v2 artifact parity and the S2 exposure wire-contract semantic gate.
//! JSON-value equality intentionally ignores whitespace, not API changes.
use serde_json::{json, Value};

fn generated() -> Result<Value, serde_json::Error> {
    serde_json::to_value(fp_api::routes::openapi_document())
}

#[test]
fn generated_rest_document_matches_committed_current_snapshot() -> Result<(), serde_json::Error> {
    let committed: Value = serde_json::from_str(include_str!(
        "../../../spec/01-api-contract.v2-openapi.json"
    ))?;
    assert_eq!(
        generated()?,
        committed,
        "regenerate with `flowplane openapi > spec/01-api-contract.v2-openapi.json`; API/schema or workspace version changes must update the snapshot in the same commit"
    );
    Ok(())
}

#[test]
fn s3_exposure_schema_declares_attachment_and_actual_cleanup_dispositions(
) -> Result<(), serde_json::Error> {
    let doc = generated()?;
    assert!(doc["paths"]["/api/v1/teams/{team}/expose"]["post"].is_object());
    assert!(doc["paths"]["/api/v1/teams/{team}/expose/{name}"]["delete"].is_object());
    let schemas = &doc["components"]["schemas"];
    assert_eq!(
        schemas["ExposureMode"]["enum"],
        json!(["created", "attached"])
    );
    assert_eq!(
        schemas["ResourceDisposition"]["enum"],
        json!(["deleted", "retained"])
    );
    assert_eq!(
        schemas["ExposeView"]["properties"]["mode"]["$ref"],
        "#/components/schemas/ExposureMode"
    );
    assert!(schemas["ExposeView"]["required"]
        .as_array()
        .is_some_and(|required| required.contains(&json!("mode"))));
    for field in [
        "cluster_disposition",
        "route_config_disposition",
        "listener_disposition",
    ] {
        assert_eq!(
            schemas["UnexposeView"]["properties"][field]["$ref"],
            "#/components/schemas/ResourceDisposition"
        );
        assert!(schemas["UnexposeView"]["required"]
            .as_array()
            .is_some_and(|required| required.contains(&json!(field))));
    }
    assert!(schemas["ExposeBody"]["properties"]["listener"].is_object());
    assert!(!schemas["ExposeBody"]["required"]
        .as_array()
        .is_some_and(|required| required.contains(&json!("listener"))));
    for selector in ["virtual_host", "route_name"] {
        assert!(
            schemas["ExposeBody"]["properties"].get(selector).is_none(),
            "virtual-host/route selectors are not part of the approved shared shortcut"
        );
    }
    assert_eq!(schemas["ExposeBody"]["additionalProperties"], false);
    Ok(())
}
