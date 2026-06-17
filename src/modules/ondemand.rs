use rayon::prelude::*;

use super::{Context, Module, ModuleConfig};
use crate::configs::ondemand::OndemandConfig;
use crate::formatter::StringFormatter;

pub fn module<'a>(context: &'a Context) -> Option<Module<'a>> {
    let root_config = context
        .config
        .get_module_config("ondemand")
        .and_then(filter_root_config);
    let config = OndemandConfig::try_load(root_config.as_ref());
    if config.disabled {
        return None;
    }

    let state = context.ondemand_state();
    let mut parts = Vec::new();

    if !state.unallowlisted_dirs.is_empty() {
        parts.push(render_approve_message(
            config.approve_format,
            state.unallowlisted_dirs.len(),
            context,
        )?);
    }

    if !state.items.is_empty() {
        let mut items: Vec<_> = state.items.iter().collect();
        items.sort_by_key(|(name, _)| *name);
        let item_parts = items
            .par_iter()
            .filter_map(|(name, config)| {
                super::custom::module_from_config(&format!("ondemand.{name}"), config, context)
                    .map(|module| module.to_string())
            })
            .filter(|item| !item.is_empty())
            .collect::<Vec<_>>();

        if !item_parts.is_empty() {
            parts.push(item_parts.join(config.item_separator));
        }
    }

    if parts.is_empty() {
        return None;
    }

    let items = parts.join(config.item_separator);
    let mut module = Module::new("ondemand", "On-demand project-local prompt items", None);
    let parsed = StringFormatter::new(config.format).and_then(|formatter| {
        formatter
            .map(|variable| match variable {
                "items" => Some(Ok(items.as_str())),
                _ => None,
            })
            .parse(None, Some(context))
    });

    match parsed {
        Ok(segments) => module.set_segments(segments),
        Err(error) => {
            log::warn!("Error in module `ondemand`:\n{error}");
            return None;
        }
    }

    Some(module)
}

/// Keep global `[ondemand.items]` reserved for the internal normalized shape.
/// Users configure on-demand items only in allowlisted `.starship/*.toml` files
/// using `[ondemand.NAME]`.
fn filter_root_config(config: &toml::Value) -> Option<toml::Value> {
    config
        .as_table()
        .map(|table| {
            table
                .iter()
                .filter(|(key, _)| key.as_str() != "items")
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect::<toml::value::Table>()
        })
        .filter(|table| !table.is_empty())
        .map(toml::Value::Table)
}

fn render_approve_message<'a>(format: &str, count: usize, context: &'a Context) -> Option<String> {
    let count = count.to_string();
    let suffix = if count == "1" { "" } else { "s" };
    let parsed = StringFormatter::new(format).and_then(|formatter| {
        formatter
            .map(|variable| match variable {
                "count" => Some(Ok(count.as_str())),
                "suffix" => Some(Ok(suffix)),
                _ => None,
            })
            .parse(None, Some(context))
    });

    match parsed {
        Ok(segments) => {
            let mut module =
                Module::new("ondemand.approve", "Unallowlisted on-demand configs", None);
            module.set_segments(segments);
            Some(module.to_string())
        }
        Err(error) => {
            log::warn!("Error in module `ondemand` approve_format:\n{error}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::test::ModuleRenderer;
    use std::fs;

    #[test]
    fn renders_warning_and_allowlisted_items() {
        let dir = tempfile::tempdir().unwrap();
        let config_home = dir.path().join("config");
        let project = dir.path().join("project");
        let service = project.join("service");
        let parent_starship = project.join(".starship");
        let child_starship = service.join(".starship");
        fs::create_dir_all(&parent_starship).unwrap();
        fs::create_dir_all(&child_starship).unwrap();
        fs::write(
            parent_starship.join("10-item.toml"),
            r#"
            [ondemand.parent]
            command = "printf parent"
            when = true
            format = "[$output](green) "
            "#,
        )
        .unwrap();
        fs::create_dir_all(&config_home).unwrap();
        fs::write(
            config_home.join("allowlist.toml"),
            format!(
                "paths = [{}]",
                toml::Value::String(
                    parent_starship
                        .canonicalize()
                        .unwrap()
                        .to_string_lossy()
                        .to_string()
                )
            ),
        )
        .unwrap();

        let actual = ModuleRenderer::new("ondemand")
            .path(&service)
            .env("STARSHIP_CONFIG_HOME", config_home.to_string_lossy())
            .config(toml::toml! {
                [ondemand]
                format = "$items"
            })
            .collect()
            .unwrap();

        assert!(actual.contains("1 new config"));
        assert!(actual.contains("parent"));
    }
}
