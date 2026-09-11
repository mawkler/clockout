use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use directories::ProjectDirs;

use crate::model::{Config, EntriesFile};

fn project_dirs() -> Result<ProjectDirs> {
    ProjectDirs::from("", "", "time").context("could not determine home directory")
}

pub fn config_path() -> Result<PathBuf> {
    Ok(project_dirs()?.config_dir().join("config.toml"))
}

pub fn data_path() -> Result<PathBuf> {
    Ok(project_dirs()?.data_dir().join("entries.json"))
}

fn write_atomic(path: &PathBuf, contents: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating directory {}", parent.display()))?;
    }
    let tmp_path = path.with_extension(format!(
        "{}.tmp",
        path.extension().and_then(|e| e.to_str()).unwrap_or("tmp")
    ));
    fs::write(&tmp_path, contents).with_context(|| format!("writing {}", tmp_path.display()))?;
    fs::rename(&tmp_path, path)
        .with_context(|| format!("renaming {} to {}", tmp_path.display(), path.display()))?;
    Ok(())
}

pub fn load_config() -> Result<Config> {
    let path = config_path()?;
    if !path.exists() {
        let cfg = Config::default();
        save_config(&cfg)?;
        return Ok(cfg);
    }
    let contents =
        fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let cfg: Config =
        toml::from_str(&contents).with_context(|| format!("parsing {}", path.display()))?;
    Ok(cfg)
}

pub fn save_config(cfg: &Config) -> Result<()> {
    let path = config_path()?;
    let contents = toml::to_string_pretty(cfg).context("serializing config")?;
    write_atomic(&path, &contents)
}

pub fn load_entries() -> Result<EntriesFile> {
    let path = data_path()?;
    if !path.exists() {
        let data = EntriesFile::default();
        save_entries(&data)?;
        return Ok(data);
    }
    let contents =
        fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let data: EntriesFile =
        serde_json::from_str(&contents).with_context(|| format!("parsing {}", path.display()))?;
    Ok(data)
}

pub fn save_entries(data: &EntriesFile) -> Result<()> {
    let path = data_path()?;
    let contents = serde_json::to_string_pretty(data).context("serializing entries")?;
    write_atomic(&path, &contents)
}
