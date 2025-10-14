use crate::errors::{FlowplaneError, Result};
use prost::Message;
use validator::ValidationError;

/// Try encoding any envoy message to ensure protobuf compatibility.
pub fn encode_check<T: Message>(message: &T, context: &str) -> Result<()> {
    if message.encode_to_vec().is_empty() {
        return Err(FlowplaneError::validation(format!(
            "{}: failed envoy-types encoding",
            context
        )));
    }
    Ok(())
}

/// Convenience helper for wrapping validation errors with context.
pub fn validation_error(message: &str) -> ValidationError {
    let mut error = ValidationError::new("invalid");
    error.message = Some(message.into());
    error
}
