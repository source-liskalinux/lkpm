use std::fs;
use std::path::PathBuf;
use crate::error::LkpmError;
use mlua::Lua;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

const SYSTEM_MIRRORLIST: &str = "/etc/lkpm.d/mirrorlist";
const SYSTEM_CONFIG: &str = "/etc/lkpm.d/config.lua";
const DEFAULT_MIRRORLIST: &str = include_str!("../etc/lkpm.d/mirrorlist");
const DEFAULT_CONFIG: &str = include_str!("../etc/lkpm.d/config.lua");

#[derive(Clone, Debug)]
pub struct Config {
    pub core_repos: Vec<String>,
    pub extra_mirrors: Vec<String>,
    pub arch: String,
    pub blocked_packages: Vec<String>,
    pub parallel_operation: usize,
    pub no_update_packages: Vec<String>,
    pub db_path: PathBuf,
    pub cache_path: PathBuf,
    pub install_root: PathBuf,
    pub log_path: PathBuf,
}

impl Config {
    pub fn ensure_private_dir(dir: &PathBuf) -> Result<(), LkpmError> {
        if let Some(parent) = dir.parent() {
            Config::ensure_dir(&parent.to_path_buf())?;
        }
        if !dir.exists() {
            fs::create_dir(dir).map_err(LkpmError::Io)?;
        }
        #[cfg(unix)]
        {
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
            .map_err(LkpmError::Io)?;
        }
        Ok(())
    }
    pub fn ensure_dir(dir: &PathBuf) -> Result<(), LkpmError> {
        if let Some(parent) = dir.parent() {
            Config::ensure_dir(&parent.to_path_buf())?;
        }
        if !dir.exists() {
            fs::create_dir(dir).map_err(LkpmError::Io)?;
        }
        #[cfg(unix)]
        {
            fs::set_permissions(dir, fs::Permissions::from_mode(0o755))
            .map_err(LkpmError::Io)?;
        }
        Ok(())
    }
    pub fn load() -> Self {
        let mut cfg = Self::defaults();
        if cfg!(test) {
            cfg.apply_mirrorlist(DEFAULT_MIRRORLIST);
            cfg.apply_config_lua(DEFAULT_CONFIG);
        } else {
            cfg.apply_system_mirrorlist();
            cfg.apply_system_config();
        }
        cfg
    }
    fn defaults() -> Self {
        Self {
            core_repos: Vec::new(),
            extra_mirrors: Vec::new(),
            arch: "x86_64".to_string(),
            blocked_packages: Vec::new(),
            no_update_packages: Vec::new(),
            parallel_operation: 5,
            db_path: PathBuf::from("/var/db/lkpm"),
            cache_path: PathBuf::from("/var/cache/lkpm"),
            install_root: PathBuf::from("/"),
            log_path: PathBuf::from("/var/log/lkpm"),
        }
    }
    fn apply_system_mirrorlist(&mut self) {
        let text = fs::read_to_string(SYSTEM_MIRRORLIST).unwrap_or_else(|_| {
            DEFAULT_MIRRORLIST.to_string()
        });
        self.apply_mirrorlist(&text);
    }
    fn apply_config_lua(&mut self, text: &str) {
        if text.trim().is_empty() {
            return;
        }
        let lua = Lua::new();
        if let Err(err) = lua.load(text).exec() {
            LkpmError::Other(format!("Failed to parse config.lua: {}", err));
            return;
        }
        let globals = lua.globals();
        if let Ok(val) = globals.get::<usize>("parallel_operation") {
            self.parallel_operation = val;
        }
        if let Ok(val) = globals.get::<String>("install_root") {
            self.install_root = PathBuf::from(val);
        }   
        if let Ok(val) = globals.get::<String>("cache_path") {
            self.cache_path = PathBuf::from(val);
            let _ = Config::ensure_private_dir(&self.cache_path);
        }
        if let Ok(val) = globals.get::<String>("log_path") {
            self.log_path = PathBuf::from(val);
            let _ = Config::ensure_private_dir(&self.log_path);
        }
        if let Ok(val) = globals.get::<String>("arch") {
            self.arch = val;
        }
        if let Ok(val) = globals.get::<Vec<String>>("blocked_packages") {
            self.blocked_packages = val;
        }
        if let Ok(val) = globals.get::<Vec<String>>("no_update") {
            self.no_update_packages = val;
        }
    }
    pub fn apply_system_config_for_root(&mut self, root: &std::path::Path) {
        let root_config = root.join("etc/lkpm.d/config.lua");
        let text = std::fs::read_to_string(&root_config)
            .or_else(|_| std::fs::read_to_string(SYSTEM_CONFIG))
            .unwrap_or_else(|_| DEFAULT_CONFIG.to_string());   
        let saved_cache = self.cache_path.clone();
        let saved_root = self.install_root.clone();
        let saved_log_path = self.log_path.clone();
        self.apply_config_lua(&text);
        self.cache_path = saved_cache;
        self.install_root = saved_root;
        self.log_path = saved_log_path;
    }
    pub fn reload_mirrorlist_for_root(&mut self) {
        let root_mirrorlist = self.install_root.join("etc/lkpm.d/mirrorlist");
        let text = fs::read_to_string(&root_mirrorlist)
            .or_else(|_| fs::read_to_string(SYSTEM_MIRRORLIST))
            .unwrap_or_else(|_| DEFAULT_MIRRORLIST.to_string());
        self.apply_mirrorlist(&text);
    }
    fn apply_system_config(&mut self) {
        let root_config = self.install_root.join("etc/lkpm.d/config.lua");
        let text = fs::read_to_string(&root_config)
            .or_else(|_| fs::read_to_string(SYSTEM_CONFIG))
            .unwrap_or_else(|_| DEFAULT_CONFIG.to_string());
        self.apply_config_lua(&text);
    }
    fn apply_mirrorlist(&mut self, text: &str) {
        let mirrorlist = Mirrorlist::parse(text);
        if !mirrorlist.core.is_empty() {
            self.core_repos = mirrorlist.core;
        }
        if !mirrorlist.extra.is_empty() {
            self.extra_mirrors = mirrorlist.extra;
        }
    }
    pub fn download_dir(&self) -> PathBuf {
        let dir = self.cache_path.join("download");
        let _ = Config::ensure_private_dir(&dir);
        dir
    }
    pub fn install_script_dir(&self) -> PathBuf {
        let dir = self.cache_path.join("tmp-script");
        let _ = Config::ensure_private_dir(&dir);
        dir
    }
    pub fn pkg_backup_dir(&self) -> PathBuf {
        let dir = self.cache_path.join("pkg-backup");
        let _ = Config::ensure_private_dir(&dir);
        dir
    }
    pub fn tmp_install_dir(&self) -> PathBuf {
        let dir = self.cache_path.join("tmp-install");
        let _ = Config::ensure_private_dir(&dir);
        dir
    }
    pub fn backup_dir(&self) -> PathBuf {
        let dir = self.install_root.join("etc/lkpm.d/backup");
        let _ = Config::ensure_dir(&dir);
        dir
    }
    pub fn update_backup_dir(&self) -> PathBuf {
        let dir = self.install_root.join("etc/lkpm.d/backup/pkg-update");
        let _ = Config::ensure_dir(&dir);
        dir
    }
    pub fn delete_backup_dir(&self) -> PathBuf {
        let dir = self.install_root.join("etc/lkpm.d/backup/pkg-delete");
        let _ = Config::ensure_dir(&dir);
        dir
    }
    pub fn lkpmsave_path(
        backup_dir: &std::path::Path,
        package_name: &str,
        file_name: &std::ffi::OsStr,
    ) -> PathBuf {
        backup_dir
            .join(package_name)
            .join(format!("{}.lkpmsave", file_name.to_string_lossy()))
    }
}

#[derive(Default)]
struct Mirrorlist {
    core: Vec<String>,
    extra: Vec<String>
}

impl Mirrorlist {
    fn parse(text: &str) -> Self {
        let mut mirrorlist = Mirrorlist::default();
        let mut section = String::new();
        for raw_line in text.lines() {
            let line = clean_line(raw_line);
            if line.is_empty() {
                continue;
            }
            if line.starts_with('[') && line.ends_with(']') {
                section = line[1..line.len() - 1].trim().to_ascii_lowercase();
                continue;
            }
            match section.as_str() {
                "core" => mirrorlist.core.push(line.to_string()),
                "extra" => mirrorlist.extra.push(line.to_string()),
                _ => {}
            }
        }
        mirrorlist
    }
}

fn clean_line(line: &str) -> &str {
    let hash_index = line.find('#').unwrap_or(line.len());
    let semicolon_index = line.find(';').unwrap_or(line.len());
    let comment_index = hash_index.min(semicolon_index);
    let without_comment = &line[..comment_index];
    without_comment.trim()
}
