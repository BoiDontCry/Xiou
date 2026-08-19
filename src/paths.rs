use std::env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

const APP_DIR: &str = "foxmail-rs";
const INDEX_FILE: &str = "mail-index.sqlite";
const LOG_FILE: &str = "foxmail-rs.log";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndexSource {
    IndexEnvironment,
    BackupEnvironment,
    Portable,
    UserData,
}

impl IndexSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::IndexEnvironment => "index_env",
            Self::BackupEnvironment => "backup_env",
            Self::Portable => "portable",
            Self::UserData => "user_data",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppPaths {
    pub index: PathBuf,
    pub log: PathBuf,
    pub log_hint: &'static str,
    pub index_source: IndexSource,
}

impl AppPaths {
    pub fn resolve() -> Self {
        let exe_dir = env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(Path::to_path_buf))
            .or_else(|| env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));
        let user_root = env::var_os("LOCALAPPDATA")
            .or_else(|| env::var_os("APPDATA"))
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .map(|path| path.join(APP_DIR));
        let portable_index = exe_dir.join("data").join(INDEX_FILE);

        resolve_paths(
            env::var_os("FOXMAIL_INDEX_PATH"),
            env::var_os("FOXMAIL_BACKUP_DIR"),
            &exe_dir,
            user_root.as_deref(),
            portable_index.is_file(),
        )
    }
}

pub fn index_is_missing(path: &Path) -> bool {
    matches!(path.try_exists(), Ok(false))
}

fn resolve_paths(
    index: Option<OsString>,
    backup_root: Option<OsString>,
    exe_dir: &Path,
    user_root: Option<&Path>,
    portable_index_exists: bool,
) -> AppPaths {
    let index = index.filter(|value| !value.is_empty()).map(PathBuf::from);
    let backup_root = backup_root
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    let portable_root = exe_dir.join("data");
    let user_data_root = user_root
        .map(|root| root.join("data"))
        .unwrap_or_else(|| portable_root.clone());

    let (index, index_source) = if let Some(index) = index {
        (index, IndexSource::IndexEnvironment)
    } else if let Some(root) = backup_root {
        (root.join(INDEX_FILE), IndexSource::BackupEnvironment)
    } else if portable_index_exists {
        (portable_root.join(INDEX_FILE), IndexSource::Portable)
    } else {
        (user_data_root.join(INDEX_FILE), IndexSource::UserData)
    };
    let log_root = if index_source == IndexSource::Portable {
        portable_root
    } else {
        user_root
            .map(Path::to_path_buf)
            .unwrap_or_else(|| exe_dir.to_path_buf())
    };

    AppPaths {
        index,
        log: log_root.join("logs").join(LOG_FILE),
        log_hint: if index_source == IndexSource::Portable {
            r"程序目录\data\logs\foxmail-rs.log"
        } else {
            r"%LOCALAPPDATA%\foxmail-rs\logs\foxmail-rs.log"
        },
        index_source,
    }
}

#[cfg(test)]
mod tests {
    use super::{index_is_missing, resolve_paths, IndexSource};
    use std::ffi::OsString;
    use std::fs;
    use std::path::{Path, PathBuf};

    #[test]
    fn explicit_index_and_backup_environment_have_highest_priority() {
        let paths = resolve_paths(
            Some(OsString::from("custom/index.sqlite")),
            Some(OsString::from("backup")),
            Path::new("app"),
            Some(Path::new("user")),
            true,
        );
        assert_eq!(paths.index, PathBuf::from("custom/index.sqlite"));
        assert_eq!(paths.index_source, IndexSource::IndexEnvironment);

        let paths = resolve_paths(
            None,
            Some(OsString::from("backup")),
            Path::new("app"),
            Some(Path::new("user")),
            true,
        );
        assert_eq!(paths.index, PathBuf::from("backup/mail-index.sqlite"));
        assert_eq!(paths.index_source, IndexSource::BackupEnvironment);
    }

    #[test]
    fn portable_index_precedes_user_data_only_when_it_exists() {
        let portable = resolve_paths(None, None, Path::new("app"), Some(Path::new("user")), true);
        assert_eq!(portable.index, PathBuf::from("app/data/mail-index.sqlite"));
        assert_eq!(portable.log, PathBuf::from("app/data/logs/foxmail-rs.log"));
        assert_eq!(portable.log_hint, r"程序目录\data\logs\foxmail-rs.log");
        assert_eq!(portable.index_source, IndexSource::Portable);

        let user = resolve_paths(None, None, Path::new("app"), Some(Path::new("user")), false);
        assert_eq!(user.index, PathBuf::from("user/data/mail-index.sqlite"));
        assert_eq!(user.log, PathBuf::from("user/logs/foxmail-rs.log"));
        assert_eq!(
            user.log_hint,
            r"%LOCALAPPDATA%\foxmail-rs\logs\foxmail-rs.log"
        );
        assert_eq!(user.index_source, IndexSource::UserData);

        let fallback = resolve_paths(None, None, Path::new("app"), None, false);
        assert_eq!(fallback.index, PathBuf::from("app/data/mail-index.sqlite"));
        assert_eq!(fallback.log, PathBuf::from("app/logs/foxmail-rs.log"));
    }

    #[test]
    fn only_a_nonexistent_path_is_missing() {
        let root = std::env::temp_dir().join(format!("foxmail-rs-paths-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        assert!(!index_is_missing(&root));
        assert!(index_is_missing(&root.join("missing.sqlite")));
        let _ = fs::remove_dir_all(root);
    }
}
