use std::fs::{self, File};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use directories::ProjectDirs;

use crate::model::{Config, EntriesFile};

fn project_dirs() -> Result<ProjectDirs> {
    ProjectDirs::from("", "", "clockout").context("could not determine home directory")
}

pub fn config_path() -> Result<PathBuf> {
    Ok(project_dirs()?.config_dir().join("config.toml"))
}

pub fn data_path() -> Result<PathBuf> {
    Ok(project_dirs()?.data_dir().join("entries.json"))
}

fn entries_lock_path() -> Result<PathBuf> {
    Ok(project_dirs()?.data_dir().join("entries.lock"))
}

/// Holds an exclusive OS advisory lock on `entries.lock` for as long as it
/// stays alive, so a load-modify-save sequence in one process can't
/// interleave with one in another (e.g. the CLI and the TUI running at the
/// same time). Released automatically when dropped.
struct EntriesLock {
    _file: File,
}

fn lock_entries() -> Result<EntriesLock> {
    let path = entries_lock_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating directory {}", parent.display()))?;
    }
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .with_context(|| format!("opening lock file {}", path.display()))?;
    file.lock()
        .with_context(|| format!("locking {}", path.display()))?;
    Ok(EntriesLock { _file: file })
}

fn write_atomic(path: &Path, contents: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating directory {}", parent.display()))?;
    }
    if path.exists() {
        let backup_path = path.with_extension(format!(
            "{}.bak",
            path.extension().and_then(|e| e.to_str()).unwrap_or("bak")
        ));
        fs::copy(path, &backup_path).with_context(|| {
            format!("backing up {} to {}", path.display(), backup_path.display())
        })?;
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

fn read_or_init_entries(path: &Path) -> Result<EntriesFile> {
    if !path.exists() {
        let data = EntriesFile::default();
        write_entries(path, &data)?;
        return Ok(data);
    }
    let contents =
        fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let data: EntriesFile =
        serde_json::from_str(&contents).with_context(|| format!("parsing {}", path.display()))?;
    Ok(data)
}

fn write_entries(path: &Path, data: &EntriesFile) -> Result<()> {
    let contents = serde_json::to_string_pretty(data).context("serializing entries")?;
    write_atomic(path, &contents)
}

/// Read-only load, for commands that only display data. Since saves are
/// atomic renames, this always sees a complete file (old or new), never a
/// torn one - no lock needed.
pub fn load_entries() -> Result<EntriesFile> {
    read_or_init_entries(&data_path()?)
}

/// Runs `f` against the current entries under an exclusive lock, then
/// persists whatever `f` did - all as one transaction, so a concurrent CLI
/// or TUI invocation can't read stale data or clobber this write. Nothing is
/// saved if `f` returns `Err`.
pub fn with_entries<T>(f: impl FnOnce(&mut EntriesFile) -> Result<T>) -> Result<T> {
    let path = data_path()?;
    let _lock = lock_entries()?;
    let mut data = read_or_init_entries(&path)?;
    let result = f(&mut data)?;
    write_entries(&path, &data)?;
    Ok(result)
}
