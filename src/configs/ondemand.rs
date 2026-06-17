use serde::{Deserialize, Serialize};

#[derive(Clone, Deserialize, Serialize)]
#[cfg_attr(
    feature = "config-schema",
    derive(schemars::JsonSchema),
    schemars(deny_unknown_fields)
)]
#[serde(default)]
pub struct OndemandConfig<'a> {
    pub disabled: bool,
    pub format: &'a str,
    pub approve_format: &'a str,
    pub item_separator: &'a str,
    pub scan_depth: usize,
}

impl Default for OndemandConfig<'_> {
    fn default() -> Self {
        Self {
            disabled: false,
            format: "$items",
            approve_format: "[$count new config$suffix (starship allowlist add)](yellow bold) ",
            item_separator: "",
            scan_depth: 8,
        }
    }
}
