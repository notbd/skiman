//! The guide `skiman import --guide` prints: how an agent prepares an import
//! folder. The tool locations and managed-elsewhere paths come from [`TOOLS`].

use crate::tools::{ManagedElsewhere, PLUGIN_MARKER, TOOLS};

const TEMPLATE: &str = include_str!("../assets/import-guide.md");

/// The guide for this skiman version.
pub fn text() -> String {
    TEMPLATE
        .replace("{{VERSION}}", env!("CARGO_PKG_VERSION"))
        .replace("{{TOOLS}}", &tool_table())
        .replace("{{MANAGED_ELSEWHERE}}", &managed_elsewhere())
}

fn tool_table() -> String {
    let mut table = String::from(
        "  | Agent tool | Tool directory | Global instruction file | Size limit |\n  |---|---|---|---|\n",
    );
    for tool in TOOLS {
        let limit = match tool.instruction_size_limit {
            Some(bytes) => format!("{bytes} bytes"),
            None => "none known".to_owned(),
        };
        table.push_str(&format!(
            "  | `{}` | `~/{}` | `~/{}` | {limit} |\n",
            tool.id, tool.tool_dir, tool.instruction_file
        ));
    }
    table.pop();
    table
}

fn managed_elsewhere() -> String {
    let mut lines: Vec<String> = TOOLS
        .iter()
        .flat_map(|tool| tool.managed_elsewhere)
        .map(|entry| match entry {
            ManagedElsewhere::Path(path) => format!("  - `~/{path}/`"),
            ManagedElsewhere::RealpathUnder(dir) => {
                format!("  - anything whose realpath is under `~/{dir}/`")
            }
        })
        .collect();
    lines.push(format!("  - any folder containing `{PLUGIN_MARKER}`"));
    lines.join("\n")
}
