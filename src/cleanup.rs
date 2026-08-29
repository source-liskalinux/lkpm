use std::path::PathBuf;
use std::fs;

pub fn cleanup(dir: &PathBuf) -> Result<(), String> {
    if dir.exists() {
        for entry in fs::read_dir(&dir).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            if path.is_dir() {
                fs::remove_dir_all(&path).map_err(|e| e.to_string())?;
            } else {
                fs::remove_file(&path).map_err(|e| e.to_string())?;
            }
        }
        fs::remove_dir(&dir).map_err(|e| e.to_string())?;
    }
    Ok(())
}