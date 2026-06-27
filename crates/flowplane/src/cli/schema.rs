//! Machine-readable CLI catalog (CLI-R-50).
//!
//! `flowplane schema` serializes the whole clap command tree to JSON so generated consumers
//! (shell completion, the future MCP tool catalog, docs) derive the CLI contract from one
//! source instead of scraping `--help`. See ADR FP-DEC-0003.

use clap::{ArgAction, Command};
use serde_json::{json, Value};

/// Catalog-format version carried *inside* `data` (distinct from the outer envelope's
/// `schemaVersion`). Bumped only on a breaking change to this schema's shape.
pub(crate) const CATALOG_VERSION: u64 = 1;

/// Build the `data` payload for `flowplane schema`: `{ catalogVersion, command }` where
/// `command` is the recursively-serialized root command tree.
pub(crate) fn cli_schema(root: &Command) -> Value {
    json!({
        "catalogVersion": CATALOG_VERSION,
        "command": command_to_value(root),
    })
}

fn command_to_value(cmd: &Command) -> Value {
    let args = cmd.get_arguments().map(arg_to_value).collect::<Vec<_>>();
    let subcommands = cmd
        .get_subcommands()
        .map(command_to_value)
        .collect::<Vec<_>>();
    json!({
        "name": cmd.get_name(),
        "about": cmd.get_about().map(|s| s.to_string()),
        "args": args,
        "subcommands": subcommands,
    })
}

fn arg_to_value(arg: &clap::Arg) -> Value {
    // A flag is a boolean switch; anything that takes a value reports its placeholder names.
    let takes_value = !matches!(
        arg.get_action(),
        ArgAction::SetTrue | ArgAction::SetFalse | ArgAction::Help | ArgAction::Version
    );
    let possible_values = arg
        .get_possible_values()
        .iter()
        .map(|pv| pv.get_name().to_string())
        .collect::<Vec<_>>();
    let defaults = arg
        .get_default_values()
        .iter()
        .map(|v| v.to_string_lossy().to_string())
        .collect::<Vec<_>>();
    let value_names = arg
        .get_value_names()
        .map(|names| names.iter().map(|n| n.to_string()).collect::<Vec<_>>())
        .unwrap_or_default();
    json!({
        "name": arg.get_id().as_str(),
        "long": arg.get_long(),
        "short": arg.get_short().map(|c| c.to_string()),
        "help": arg.get_help().map(|s| s.to_string()),
        "required": arg.is_required_set(),
        "global": arg.is_global_set(),
        "takesValue": takes_value,
        "type": value_type(arg, takes_value, !possible_values.is_empty()),
        "valueNames": value_names,
        "possibleValues": possible_values,
        "defaults": defaults,
    })
}

/// A coarse value-type label for an argument (CLI-R-50: "types/enums"). A flag is `boolean`;
/// an arg with possible values is `enum`; otherwise the value-parser's `TypeId` is matched
/// against the known scalar types (this comparison is reliable in release builds, unlike the
/// parser's debug-only type name). Unknown parsers fall back to `string`.
fn value_type(arg: &clap::Arg, takes_value: bool, is_enum: bool) -> &'static str {
    if !takes_value {
        return "boolean";
    }
    if is_enum {
        return "enum";
    }
    let tid = arg.get_value_parser().type_id();
    for (label, parser) in known_value_parsers() {
        if tid == parser.type_id() {
            return label;
        }
    }
    "string"
}

#[allow(clippy::useless_conversion)] // `.into()` is needed for the typed parsers; identity for the rest.
fn known_value_parsers() -> Vec<(&'static str, clap::builder::ValueParser)> {
    use clap::value_parser;
    vec![
        ("integer", value_parser!(i64).into()),
        ("integer", value_parser!(i32).into()),
        ("integer", value_parser!(u64).into()),
        ("integer", value_parser!(u32).into()),
        ("integer", value_parser!(u16).into()),
        ("integer", value_parser!(u8).into()),
        ("number", value_parser!(f64).into()),
        ("boolean", value_parser!(bool).into()),
        ("path", value_parser!(std::path::PathBuf).into()),
        ("string", value_parser!(String).into()),
    ]
}

/// The set of top-level subcommand names in the schema, for the drift guard.
#[cfg(test)]
pub(crate) fn top_level_command_names(schema: &Value) -> Vec<String> {
    schema["command"]["subcommands"]
        .as_array()
        .map(|subs| {
            subs.iter()
                .filter_map(|s| s["name"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use clap::{value_parser, Arg, ArgAction, Command};

    #[test]
    fn arg_value_types_distinguish_string_integer_bool_enum() {
        let cmd = Command::new("t")
            .arg(Arg::new("name"))
            .arg(
                Arg::new("count")
                    .long("count")
                    .value_parser(value_parser!(i64)),
            )
            .arg(
                Arg::new("port")
                    .long("port")
                    .value_parser(value_parser!(u16)),
            )
            .arg(Arg::new("flag").long("flag").action(ArgAction::SetTrue))
            .arg(Arg::new("mode").long("mode").value_parser(["dev", "mtls"]));
        let schema = cli_schema(&cmd);
        let args = schema["command"]["args"].as_array().unwrap();
        let ty = |name: &str| {
            args.iter()
                .find(|a| a["name"] == name)
                .and_then(|a| a["type"].as_str())
                .unwrap()
                .to_string()
        };
        assert_eq!(ty("name"), "string");
        assert_eq!(ty("count"), "integer");
        assert_eq!(ty("port"), "integer");
        assert_eq!(ty("flag"), "boolean");
        assert_eq!(ty("mode"), "enum");
        // enum values are still enumerated under possibleValues.
        let mode = args.iter().find(|a| a["name"] == "mode").unwrap();
        assert_eq!(mode["possibleValues"], serde_json::json!(["dev", "mtls"]));
    }
}
