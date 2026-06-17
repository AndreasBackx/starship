use crate::config::{ConfigSources, ModuleConfig};
use crate::configs::ondemand::OndemandConfig;
use crate::utils;

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

pub const DIR_NAME: &str = ".starship";

/// Cached per-prompt result of scanning for project-local `.starship` directories.
///
/// Local files are intentionally not merged into the main Starship config. They
/// can only contribute `[ondemand.NAME]` item definitions after their containing
/// `.starship` directory has been explicitly allowlisted.
#[derive(Debug, Clone, Default)]
pub struct OndemandState {
    pub allowlisted_dirs: Vec<PathBuf>,
    pub unallowlisted_dirs: Vec<PathBuf>,
    pub items: toml::value::Table,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
struct Allowlist {
    paths: Vec<PathBuf>,
}

pub fn allowlist_path(config_sources: &ConfigSources) -> Option<PathBuf> {
    config_sources
        .user_config_home
        .as_ref()
        .map(|dir| dir.join("allowlist.toml"))
}

pub fn load_allowlist(config_sources: &ConfigSources) -> HashSet<PathBuf> {
    let Some(path) = allowlist_path(config_sources) else {
        return HashSet::new();
    };
    let Ok(content) = fs::read_to_string(&path) else {
        return HashSet::new();
    };
    match toml::from_str::<Allowlist>(&content) {
        Ok(allowlist) => allowlist.paths.into_iter().collect(),
        Err(error) => {
            log::error!("Unable to parse allowlist file {}: {error}", path.display());
            HashSet::new()
        }
    }
}

fn save_allowlist(config_sources: &ConfigSources, paths: HashSet<PathBuf>) -> Result<(), String> {
    let path = allowlist_path(config_sources)
        .ok_or_else(|| "Unable to determine STARSHIP_CONFIG_HOME".to_string())?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let mut paths: Vec<_> = paths.into_iter().collect();
    paths.sort();
    let content =
        toml::to_string_pretty(&Allowlist { paths }).map_err(|error| error.to_string())?;
    utils::write_file_atomic(path, content, true).map_err(|error| error.to_string())
}

pub fn add_allowlist_path(config_sources: &ConfigSources, path: &Path) -> Result<PathBuf, String> {
    let ondemand_dir = resolve_ondemand_dir(path)?;
    let mut allowlist = load_allowlist(config_sources);
    allowlist.insert(ondemand_dir.clone());
    save_allowlist(config_sources, allowlist)?;
    Ok(ondemand_dir)
}

pub fn remove_allowlist_path(
    config_sources: &ConfigSources,
    path: &Path,
) -> Result<PathBuf, String> {
    let ondemand_dir = resolve_ondemand_dir(path)?;
    let mut allowlist = load_allowlist(config_sources);
    if !allowlist.remove(&ondemand_dir) {
        return Err(format!("{} is not allowlisted", ondemand_dir.display()));
    }
    save_allowlist(config_sources, allowlist)?;
    Ok(ondemand_dir)
}

pub fn list_allowlist_paths(config_sources: &ConfigSources) -> Vec<PathBuf> {
    let mut paths: Vec<_> = load_allowlist(config_sources).into_iter().collect();
    paths.sort();
    paths
}

pub fn resolve_ondemand_dir(path: &Path) -> Result<PathBuf, String> {
    let path = path.canonicalize().map_err(|error| error.to_string())?;
    if path.file_name() == Some(OsStr::new(DIR_NAME)) && path.is_dir() {
        return Ok(path);
    }
    let child = path.join(DIR_NAME);
    if child.is_dir() {
        return child.canonicalize().map_err(|error| error.to_string());
    }
    Err(format!(
        "No {DIR_NAME} directory found at {} or its direct child",
        path.display()
    ))
}

pub fn discover(
    current_dir: &Path,
    config_sources: &ConfigSources,
    root_config: &crate::configs::StarshipRootConfig,
) -> OndemandState {
    let ondemand_config = root_config.ondemand.as_ref().map(|table| {
        toml::Value::Table(
            table
                .iter()
                .filter(|(key, _)| key.as_str() != "items")
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        )
    });
    let config = OndemandConfig::try_load(ondemand_config.as_ref());
    let allowlist = load_allowlist(config_sources);
    let mut state = OndemandState::default();

    let mut dirs = Vec::new();
    // The scan is bounded by `[ondemand].scan_depth` and cached by `Context`, so
    // prompt rendering does not repeatedly walk ancestors or re-read TOML files.
    for ancestor in current_dir
        .ancestors()
        .take(config.scan_depth.saturating_add(1))
    {
        let dir = ancestor.join(DIR_NAME);
        if !dir.is_dir() {
            continue;
        }
        let dir = dir.canonicalize().unwrap_or(dir);
        dirs.push(dir);
    }
    dirs.reverse();

    for dir in dirs {
        if allowlist.contains(&dir) {
            merge_items_from_dir(&dir, &mut state.items);
            state.allowlisted_dirs.push(dir);
        } else {
            state.unallowlisted_dirs.push(dir);
        }
    }

    state
}

fn merge_items_from_dir(dir: &Path, items: &mut toml::value::Table) {
    for file in sorted_toml_files(dir) {
        let Ok(content) = fs::read_to_string(&file) else {
            continue;
        };
        let parsed = match toml::from_str::<toml::Value>(&content) {
            Ok(parsed) => parsed,
            Err(error) => {
                log::error!(
                    "Unable to parse ondemand config file {}: {error}",
                    file.display()
                );
                continue;
            }
        };
        merge_items_from_value(&file, parsed, items);
    }
}

fn merge_items_from_value(file: &Path, value: toml::Value, items: &mut toml::value::Table) {
    let Some(table) = value.as_table() else {
        log::warn!("Ignoring non-table ondemand config {}", file.display());
        return;
    };

    for (key, value) in table {
        if key != "ondemand" {
            log::warn!(
                "Ignoring unsupported ondemand config section {key:?} in {}",
                file.display()
            );
            continue;
        }
        let Some(ondemand) = value.as_table() else {
            log::warn!("Ignoring non-table [ondemand] in {}", file.display());
            continue;
        };
        for (name, item) in ondemand {
            // `[ondemand.items]` is reserved for the normalized internal shape;
            // project files use `[ondemand.NAME]` and cannot configure root
            // `[ondemand]` module settings.
            if !item.is_table() || name == "items" {
                log::warn!(
                    "Ignoring unsupported ondemand config key {name:?} in {}",
                    file.display()
                );
                continue;
            }
            if let Some(existing) = items.get_mut(name) {
                merge_toml(existing, item.clone());
            } else {
                items.insert(name.clone(), item.clone());
            }
        }
    }
}

fn sorted_toml_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<_> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "toml")
        })
        .filter(|path| path.is_file())
        .collect();
    files.sort();
    files
}

fn merge_toml(base: &mut toml::Value, overlay: toml::Value) {
    match (base, overlay) {
        (toml::Value::Table(base), toml::Value::Table(overlay)) => {
            for (key, value) in overlay {
                if let Some(base_value) = base.get_mut(&key) {
                    merge_toml(base_value, value);
                } else {
                    base.insert(key, value);
                }
            }
        }
        (base, overlay) => *base = overlay,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::configs::StarshipRootConfig;

    fn write(path: &Path, content: &str) {
        fs::write(path, content).unwrap();
    }

    #[test]
    fn resolve_accepts_starship_dir_or_direct_parent() {
        let dir = tempfile::tempdir().unwrap();
        let starship = dir.path().join(DIR_NAME);
        fs::create_dir(&starship).unwrap();

        assert_eq!(
            resolve_ondemand_dir(dir.path()).unwrap(),
            starship.canonicalize().unwrap()
        );
        assert_eq!(
            resolve_ondemand_dir(&starship).unwrap(),
            starship.canonicalize().unwrap()
        );
    }

    #[test]
    fn resolve_rejects_greater_parent() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        fs::create_dir(&project).unwrap();
        fs::create_dir(project.join(DIR_NAME)).unwrap();

        assert!(resolve_ondemand_dir(dir.path()).is_err());
    }

    #[test]
    fn discover_splits_allowlisted_and_unallowlisted_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let config_home = dir.path().join("config");
        let project = dir.path().join("project");
        let service = project.join("service");
        let parent_starship = project.join(DIR_NAME);
        let child_starship = service.join(DIR_NAME);
        fs::create_dir_all(&parent_starship).unwrap();
        fs::create_dir_all(&child_starship).unwrap();
        write(
            &parent_starship.join("10-item.toml"),
            r#"
            [ondemand.parent]
            command = "printf parent"
            when = true
            "#,
        );

        let sources = ConfigSources {
            explicit_config: None,
            user_config_home: Some(config_home.clone()),
            user_config_file: None,
            user_conf_d: None,
            system_conf_dirs: Vec::new(),
            legacy_config_file: None,
        };
        add_allowlist_path(&sources, &parent_starship).unwrap();

        let state = discover(&service, &sources, &StarshipRootConfig::default());
        assert_eq!(
            state.allowlisted_dirs,
            vec![parent_starship.canonicalize().unwrap()]
        );
        assert_eq!(
            state.unallowlisted_dirs,
            vec![child_starship.canonicalize().unwrap()]
        );
        assert!(state.items.contains_key("parent"));
    }
}
