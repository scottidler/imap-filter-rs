use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;

pub fn load_last_uid() -> io::Result<Option<u32>> {
    let path = last_uid_path();
    match fs::read_to_string(&path) {
        Ok(content) => {
            let trimmed = content.trim();
            if trimmed.is_empty() {
                Ok(None)
            } else {
                trimmed.parse::<u32>().map(Some).map_err(|e| {
                    io::Error::new(io::ErrorKind::InvalidData, format!("Invalid UID: {e}"))
                })
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

pub fn save_last_uid(uid: u32) -> io::Result<()> {
    let path = last_uid_path();
    let mut file = fs::File::create(path)?;
    writeln!(file, "{}", uid)
}

fn last_uid_path() -> PathBuf {
    let mut path = dirs::config_dir().unwrap_or_else(|| PathBuf::from("~/.config"));
    path.push("imap-filter/.last_uid");
    path
}
