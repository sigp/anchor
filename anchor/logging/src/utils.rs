use std::collections::HashSet;

use tracing_subscriber::filter::FilterFn;
use workspace_members::workspace_crates;

pub(crate) const WORKSPACE_CRATES: &[&str] = workspace_crates!();
pub(crate) const LIGHTHOUSE_CRATES: &[&str] = &[
    "beacon_node_fallback",
    "slashing_protection",
    "task_executor",
    "validator_services",
];

/// Constructs a filter which only permits logging from crates which are members of the workspace.
pub fn build_workspace_filter()
-> Result<FilterFn<impl Fn(&tracing::Metadata) -> bool + Clone>, String> {
    let mut workspace_crates: HashSet<&str> = WORKSPACE_CRATES.iter().copied().collect();
    workspace_crates.extend(LIGHTHOUSE_CRATES.iter().copied());

    Ok(tracing_subscriber::filter::FilterFn::new(move |metadata| {
        let target_crate = metadata.target().split("::").next().unwrap_or("");
        workspace_crates.contains(target_crate)
    }))
}
