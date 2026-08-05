//! Runtime configuration and portable data directory layout.
//!
//! A small [`bigshot.json`](CONFIG_FILE) in the user config directory records an
//! optional custom data path. On first boot the file is created with the
//! default (no override). Changing the path later does **not** move existing
//! files — stop the server, move `data/` yourself, then update the path
//! (`--set-data-dir` / `--clear-data-dir`, or edit the JSON).
//!
//! Platform locations for the config directory:
//! - Windows: `%APPDATA%\bigshot-voter\`
//! - macOS: `~/Library/Application Support/bigshot-voter/`
//! - Linux / other: `$XDG_CONFIG_HOME/bigshot-voter/` or `~/.config/bigshot-voter/`

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::Parser;
use rand::RngCore;
use serde::{Deserialize, Serialize};

const SECRET_LEN: usize = 32;
const DEFAULT_MAX_UPLOAD: u64 = 32 * 1024 * 1024;

/// Config file name stored under the user config directory.
pub const CONFIG_FILE: &str = "bigshot.json";

/// Application folder name under the platform config root.
pub const APP_CONFIG_DIR: &str = "bigshot-voter";

#[derive(Debug, Clone, Parser)]
#[command(name = "bigshot-voter", about = "Big-Shot Voting server")]
pub struct Cli {
    /// Directory for SQLite DB, secret key, and uploads.
    /// Overrides `data_dir` in `bigshot.json` for this process only.
    /// Defaults to `data/` next to the server executable.
    #[arg(long, env = "BIGSHOT_DATA_DIR", value_name = "PATH")]
    pub data_dir: Option<PathBuf>,

    /// Write `data_dir` into the user config file and exit (does not start the server).
    /// Relative paths are stored absolute (resolved against the current working directory).
    #[arg(long, value_name = "PATH")]
    pub set_data_dir: Option<PathBuf>,

    /// Clear the saved `data_dir` override (back to `{exe_dir}/data`) and exit.
    #[arg(long, conflicts_with = "set_data_dir")]
    pub clear_data_dir: bool,

    /// Bind address (host:port).
    #[arg(long, env = "BIGSHOT_BIND", default_value = "127.0.0.1:17843")]
    pub bind: String,

    /// Maximum upload size in bytes.
    #[arg(long, env = "BIGSHOT_MAX_UPLOAD", default_value_t = DEFAULT_MAX_UPLOAD)]
    pub max_upload_bytes: u64,
}

/// On-disk settings in the user config directory. Generated on first start.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileConfig {
    /// Optional path for the database, secret key, and uploads.
    ///
    /// - `null` or omitted / empty → use `data/` next to the executable
    /// - absolute path → use that directory as-is
    /// - relative path → resolved against the directory containing this file
    ///
    /// Changing this does not move data; relocate the folder yourself first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_dir: Option<String>,
}

impl Default for FileConfig {
    fn default() -> Self {
        Self { data_dir: None }
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    pub data_dir: PathBuf,
    pub db_path: PathBuf,
    pub uploads_dir: PathBuf,
    pub secret: Vec<u8>,
    pub bind: String,
    pub max_upload_bytes: u64,
    /// Path of the JSON config in the user config directory (for logging).
    pub config_file: PathBuf,
}

impl Config {
    pub fn from_cli(cli: Cli) -> Result<Self> {
        let exe_dir = executable_dir()?;
        let config_file = resolve_config_file_path(&exe_dir)?;
        let file_cfg = load_or_create_file_config(&config_file)?;
        let config_dir = config_file
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));

        let data_dir = match cli.data_dir {
            Some(p) => p,
            None => resolve_data_dir(&file_cfg, &exe_dir, &config_dir),
        };

        fs::create_dir_all(&data_dir)
            .with_context(|| format!("create data dir {}", data_dir.display()))?;

        let uploads_dir = data_dir.join("uploads");
        fs::create_dir_all(&uploads_dir)
            .with_context(|| format!("create uploads dir {}", uploads_dir.display()))?;

        let secret = load_or_create_secret(&data_dir.join("secret.key"))?;
        let db_path = data_dir.join("bigshot.db");

        Ok(Self {
            data_dir,
            db_path,
            uploads_dir,
            secret,
            bind: cli.bind,
            max_upload_bytes: cli.max_upload_bytes,
            config_file,
        })
    }
}

/// Directory that holds the executable (falls back to CWD).
fn executable_dir() -> Result<PathBuf> {
    if let Ok(exe) = std::env::current_exe()
        && let Some(parent) = exe.parent()
    {
        return Ok(parent.to_path_buf());
    }
    std::env::current_dir().context("determine current working directory")
}

/// Platform user config directory for this app (`…/bigshot-voter`).
pub fn user_config_dir() -> Result<PathBuf> {
    let root = platform_config_home()?;
    Ok(root.join(APP_CONFIG_DIR))
}

fn platform_config_home() -> Result<PathBuf> {
    #[cfg(windows)]
    {
        if let Ok(appdata) = std::env::var("APPDATA") {
            let p = PathBuf::from(appdata);
            if !p.as_os_str().is_empty() {
                return Ok(p);
            }
        }
        let home = std::env::var_os("USERPROFILE")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
            .context("determine user home (USERPROFILE / HOME)")?;
        return Ok(home.join("AppData").join("Roaming"));
    }

    #[cfg(target_os = "macos")]
    {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .context("determine user home (HOME)")?;
        return Ok(home.join("Library").join("Application Support"));
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    {
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            let p = PathBuf::from(xdg);
            if !p.as_os_str().is_empty() {
                return Ok(p);
            }
        }
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .context("determine user home (HOME)")?;
        return Ok(home.join(".config"));
    }
}

/// Path of `bigshot.json` under the user config directory.
///
/// If a legacy file still sits next to the executable and the user-dir file is
/// missing, it is migrated once so existing installs keep working.
fn resolve_config_file_path(exe_dir: &Path) -> Result<PathBuf> {
    let config_dir = user_config_dir()?;
    let path = config_dir.join(CONFIG_FILE);
    let legacy = exe_dir.join(CONFIG_FILE);
    if migrate_legacy_config(&legacy, &path)? {
        tracing::info!(
            from = %legacy.display(),
            to = %path.display(),
            "migrated config to user config directory"
        );
    }
    Ok(path)
}

/// Move a next-to-exe config into the user config path when the latter is absent.
///
/// Returns `true` if a migration was performed.
fn migrate_legacy_config(legacy: &Path, dest: &Path) -> Result<bool> {
    if dest.exists() || !legacy.is_file() {
        return Ok(false);
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create config dir for migration {}", parent.display()))?;
    }
    // Prefer rename; fall back to copy+remove when the move crosses devices.
    match fs::rename(legacy, dest) {
        Ok(()) => Ok(true),
        Err(_) => {
            fs::copy(legacy, dest).with_context(|| {
                format!(
                    "migrate config from {} to {}",
                    legacy.display(),
                    dest.display()
                )
            })?;
            let _ = fs::remove_file(legacy);
            Ok(true)
        }
    }
}

/// Read `bigshot.json`, or write a default file on first boot.
pub fn load_or_create_file_config(path: &Path) -> Result<FileConfig> {
    if path.exists() {
        let text = fs::read_to_string(path)
            .with_context(|| format!("read config {}", path.display()))?;
        let cfg: FileConfig = serde_json::from_str(&text)
            .with_context(|| format!("parse config {}", path.display()))?;
        return Ok(cfg);
    }

    write_file_config(path, &FileConfig::default())?;
    tracing::info!(path = %path.display(), "created default config");
    Ok(FileConfig::default())
}

/// Write `bigshot.json` in a stable, hand-editable form (`data_dir` always present).
pub fn write_file_config(path: &Path, cfg: &FileConfig) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create config dir {}", parent.display()))?;
    }

    // Keep the key visible even when null (serde would skip `Option::None`).
    let value = serde_json::json!({
        "data_dir": cfg.data_dir.as_ref().map(|s| s.trim()).filter(|s| !s.is_empty()),
    });
    let body = format!("{}\n", serde_json::to_string_pretty(&value)?);
    fs::write(path, body).with_context(|| format!("write config {}", path.display()))?;
    Ok(())
}

/// Result of [`set_persisted_data_dir`] / [`clear_persisted_data_dir`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigWriteResult {
    pub config_file: PathBuf,
    /// Path stored in the file, or `None` when cleared (default next to the executable).
    pub data_dir: Option<String>,
}

/// Persist a data directory path into the user config file (does not start the server).
///
/// Relative paths are resolved against the current working directory and stored
/// as absolute paths so later runs do not depend on CWD.
pub fn set_persisted_data_dir(path: impl AsRef<Path>) -> Result<ConfigWriteResult> {
    let path = path.as_ref();
    if path.as_os_str().is_empty() {
        bail!("data directory path must not be empty (use --clear-data-dir to reset)");
    }

    let stored = absolutize_path(path)?;
    // Prefer a clean display string; on Windows this keeps backslashes as given by the OS.
    let stored_str = stored.to_string_lossy().into_owned();

    let exe_dir = executable_dir()?;
    let config_file = resolve_config_file_path(&exe_dir)?;
    let mut cfg = load_or_create_file_config(&config_file)?;
    cfg.data_dir = Some(stored_str.clone());
    write_file_config(&config_file, &cfg)?;

    Ok(ConfigWriteResult {
        config_file,
        data_dir: Some(stored_str),
    })
}

/// Clear the saved data directory override so the server uses `{exe_dir}/data`.
pub fn clear_persisted_data_dir() -> Result<ConfigWriteResult> {
    let exe_dir = executable_dir()?;
    let config_file = resolve_config_file_path(&exe_dir)?;
    let mut cfg = load_or_create_file_config(&config_file)?;
    cfg.data_dir = None;
    write_file_config(&config_file, &cfg)?;

    Ok(ConfigWriteResult {
        config_file,
        data_dir: None,
    })
}

/// Resolve a path to absolute without requiring it to exist on disk.
fn absolutize_path(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    let cwd = std::env::current_dir().context("determine current working directory")?;
    Ok(cwd.join(path))
}

/// Pick the data directory from the file config, or `{exe_dir}/data`.
///
/// Relative `data_dir` values are resolved against `config_dir` (the folder that
/// holds `bigshot.json`). Absolute paths are used as-is.
fn resolve_data_dir(file: &FileConfig, exe_dir: &Path, config_dir: &Path) -> PathBuf {
    match file.data_dir.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        None => exe_dir.join("data"),
        Some(s) => {
            let p = PathBuf::from(s);
            if p.is_absolute() {
                p
            } else {
                config_dir.join(p)
            }
        }
    }
}

fn load_or_create_secret(path: &Path) -> Result<Vec<u8>> {
    if path.exists() {
        let bytes = fs::read(path).with_context(|| format!("read secret {}", path.display()))?;
        if bytes.len() < SECRET_LEN {
            bail!(
                "secret key at {} is too short (need at least {SECRET_LEN} bytes)",
                path.display()
            );
        }
        return Ok(bytes);
    }

    let mut key = vec![0u8; SECRET_LEN];
    rand::thread_rng().fill_bytes(&mut key);
    fs::write(path, &key).with_context(|| format!("write secret {}", path.display()))?;

    // Best-effort restrict permissions on Unix; no-op on Windows.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }

    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_default_config_on_first_read() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(CONFIG_FILE);
        assert!(!path.exists());

        let cfg = load_or_create_file_config(&path).unwrap();
        assert_eq!(cfg, FileConfig::default());
        assert!(path.is_file());

        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("data_dir"));
        // Second read returns the same defaults.
        let again = load_or_create_file_config(&path).unwrap();
        assert_eq!(again, FileConfig::default());
    }

    #[test]
    fn empty_or_null_data_dir_means_default_next_to_exe() {
        let exe = PathBuf::from("/opt/bigshot");
        let cfg_dir = PathBuf::from("/home/user/.config/bigshot-voter");
        assert_eq!(
            resolve_data_dir(&FileConfig { data_dir: None }, &exe, &cfg_dir),
            PathBuf::from("/opt/bigshot/data")
        );
        assert_eq!(
            resolve_data_dir(
                &FileConfig {
                    data_dir: Some(String::new()),
                },
                &exe,
                &cfg_dir
            ),
            PathBuf::from("/opt/bigshot/data")
        );
        assert_eq!(
            resolve_data_dir(
                &FileConfig {
                    data_dir: Some("   ".into()),
                },
                &exe,
                &cfg_dir
            ),
            PathBuf::from("/opt/bigshot/data")
        );
    }

    #[test]
    fn relative_data_dir_is_joined_to_config_dir() {
        let exe = PathBuf::from("/opt/bigshot");
        let cfg_dir = PathBuf::from("/home/user/.config/bigshot-voter");
        let cfg = FileConfig {
            data_dir: Some("storage".into()),
        };
        assert_eq!(
            resolve_data_dir(&cfg, &exe, &cfg_dir),
            PathBuf::from("/home/user/.config/bigshot-voter/storage")
        );
    }

    #[test]
    fn absolute_data_dir_is_used_as_is() {
        let exe = PathBuf::from("/opt/bigshot");
        let cfg_dir = PathBuf::from("/home/user/.config/bigshot-voter");
        #[cfg(windows)]
        let abs = r"D:\bigshot-data";
        #[cfg(not(windows))]
        let abs = "/var/lib/bigshot";
        let cfg = FileConfig {
            data_dir: Some(abs.into()),
        };
        assert_eq!(resolve_data_dir(&cfg, &exe, &cfg_dir), PathBuf::from(abs));
    }

    #[test]
    fn creates_parent_dirs_when_writing_config() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nested").join("cfg").join(CONFIG_FILE);
        assert!(!path.exists());
        let cfg = load_or_create_file_config(&path).unwrap();
        assert_eq!(cfg, FileConfig::default());
        assert!(path.is_file());
    }

    #[test]
    fn migrates_legacy_config_next_to_exe() {
        let tmp = tempfile::tempdir().unwrap();
        let exe_dir = tmp.path().join("bin");
        let dest_dir = tmp.path().join("user-config").join(APP_CONFIG_DIR);
        fs::create_dir_all(&exe_dir).unwrap();
        let legacy = exe_dir.join(CONFIG_FILE);
        let dest = dest_dir.join(CONFIG_FILE);
        fs::write(&legacy, "{\n  \"data_dir\": \"my-data\"\n}\n").unwrap();

        assert!(migrate_legacy_config(&legacy, &dest).unwrap());
        assert!(dest.is_file());
        assert!(!legacy.exists());
        let cfg = load_or_create_file_config(&dest).unwrap();
        assert_eq!(cfg.data_dir.as_deref(), Some("my-data"));

        // Second call is a no-op once the destination exists.
        fs::write(&legacy, "{}\n").unwrap();
        assert!(!migrate_legacy_config(&legacy, &dest).unwrap());
        assert_eq!(
            load_or_create_file_config(&dest).unwrap().data_dir.as_deref(),
            Some("my-data")
        );
    }

    #[test]
    fn parses_hand_edited_config() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(CONFIG_FILE);
        fs::write(
            &path,
            r#"{
  "data_dir": "my-data"
}
"#,
        )
        .unwrap();
        let cfg = load_or_create_file_config(&path).unwrap();
        assert_eq!(cfg.data_dir.as_deref(), Some("my-data"));
    }

    #[test]
    fn write_file_config_keeps_null_data_dir_visible() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(CONFIG_FILE);
        write_file_config(&path, &FileConfig::default()).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"data_dir\": null") || text.contains("\"data_dir\":null"));
    }

    #[test]
    fn write_file_config_round_trips_path() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(CONFIG_FILE);
        #[cfg(windows)]
        let data = r"F:\Servers\Big-Shot";
        #[cfg(not(windows))]
        let data = "/var/lib/bigshot";
        write_file_config(
            &path,
            &FileConfig {
                data_dir: Some(data.into()),
            },
        )
        .unwrap();
        let cfg = load_or_create_file_config(&path).unwrap();
        assert_eq!(cfg.data_dir.as_deref(), Some(data));
    }

    #[test]
    fn absolutize_leaves_absolute_paths() {
        #[cfg(windows)]
        let abs = PathBuf::from(r"F:\Servers\Big-Shot");
        #[cfg(not(windows))]
        let abs = PathBuf::from("/var/lib/bigshot");
        assert_eq!(absolutize_path(&abs).unwrap(), abs);
    }

    #[test]
    fn absolutize_joins_relative_to_cwd() {
        let cwd = std::env::current_dir().unwrap();
        let got = absolutize_path(Path::new("relative-data")).unwrap();
        assert_eq!(got, cwd.join("relative-data"));
    }
}
