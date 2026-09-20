use serde::{Deserialize, Serialize};
use std::env;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::PathBuf;

const APP_DIR: &str = "foxmail-rs";

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct DraftData {
    pub to: String,
    pub cc: String,
    pub subject: String,
    pub body: String,
}

pub fn load(key: &str) -> io::Result<Option<DraftData>> {
    let path = path_for(key)?;
    if !path.is_file() {
        return Ok(None);
    }
    serde_json::from_slice(&fs::read(path)?)
        .map(Some)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

pub fn save(key: &str, draft: &DraftData) -> io::Result<()> {
    let path = path_for(key)?;
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "草稿路径缺少父目录"))?;
    fs::create_dir_all(parent)?;
    let contents = serde_json::to_vec(draft)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    let mut file = File::create(&temporary)?;
    file.write_all(&contents)?;
    file.sync_all()?;
    drop(file);
    if path.exists() {
        fs::remove_file(&path)?;
    }
    fs::rename(temporary, path)
}

pub fn remove(key: &str) -> io::Result<()> {
    let path = path_for(key)?;
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn path_for(key: &str) -> io::Result<PathBuf> {
    let base = env::var_os("LOCALAPPDATA")
        .or_else(|| env::var_os("APPDATA"))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "LOCALAPPDATA/APPDATA 未设置"))?;
    Ok(PathBuf::from(base)
        .join(APP_DIR)
        .join("drafts")
        .join(format!("{}.json", safe_key(key))))
}

fn safe_key(key: &str) -> String {
    key.chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::safe_key;

    #[test]
    fn normalizes_draft_keys_for_windows_paths() {
        assert_eq!(safe_key("reply:42/中文"), "reply_42___");
    }
}
