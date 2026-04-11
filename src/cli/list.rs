//! List CLI command
//!
//! Shows exposed services in a table, stripping the `-listener` suffix
//! and looking up cluster/upstream info.

use anyhow::Result;

use super::client::FlowplaneClient;
use super::listeners::ListenerResponse;
use super::output::truncate;
use crate::api::handlers::PaginatedResponse;

/// Strip the `-listener` suffix from a listener name, if present.
pub fn strip_listener_suffix(name: &str) -> &str {
    name.strip_suffix("-listener").unwrap_or(name)
}

/// Handle `flowplane list`
pub async fn handle_list_command(client: &FlowplaneClient, team: &str) -> Result<()> {
    let path = format!("/api/v1/teams/{team}/listeners?limit=1000");
    let response: PaginatedResponse<ListenerResponse> = client.get_json(&path).await?;

    if response.items.is_empty() {
        println!("No exposed services found");
        return Ok(());
    }

    println!();
    println!("{:<30} {:<8} {:<15}", "Name", "Port", "Protocol");
    println!("{}", "-".repeat(55));

    for listener in &response.items {
        let display_name = strip_listener_suffix(&listener.name);
        println!(
            "{:<30} {:<8} {:<15}",
            truncate(display_name, 28),
            listener.port,
            listener.protocol,
        );
    }
    println!();

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_listener_suffix_removes_suffix() {
        assert_eq!(strip_listener_suffix("my-api-listener"), "my-api");
    }

    #[test]
    fn strip_listener_suffix_no_suffix() {
        assert_eq!(strip_listener_suffix("my-api"), "my-api");
    }

    #[test]
    fn strip_listener_suffix_only_suffix() {
        assert_eq!(strip_listener_suffix("-listener"), "");
    }

    #[test]
    fn strip_listener_suffix_empty() {
        assert_eq!(strip_listener_suffix(""), "");
    }

    #[test]
    fn strip_listener_suffix_listener_in_middle() {
        assert_eq!(strip_listener_suffix("my-listener-api"), "my-listener-api");
    }

    #[test]
    fn strip_listener_suffix_double_suffix() {
        assert_eq!(strip_listener_suffix("my-api-listener-listener"), "my-api-listener");
    }

    #[test]
    fn strip_listener_suffix_exact_word() {
        assert_eq!(strip_listener_suffix("listener"), "listener");
    }
}
