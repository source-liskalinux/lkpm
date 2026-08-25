use crate::config::Config;
use crate::error::LkpmError;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

fn default_source_kind() -> String {
    "unknown".into()
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct InstalledPackage {
    pub name: String,
    pub version: String,
    pub source: String,
    #[serde(default = "default_source_kind")]
    pub source_kind: String,
    pub package_path: PathBuf,
    pub checksum: String,
    pub files: Vec<PathBuf>,
    #[serde(default)]
    pub requires: Vec<String>,
    #[serde(default)]
    pub optdepends: Vec<String>,
    #[serde(default)]
    pub conflicts: Vec<String>,
    #[serde(default)]
    pub provides: Vec<String>,
    #[serde(default)]
    pub backups: Vec<PathBuf>,
    #[serde(default)]
    pub backups_hashes: HashMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct Database {
    pub db_file: PathBuf,
}

impl Database {
    fn ensure_db_dir(db_path: &PathBuf) -> Result<(), LkpmError> {
        if !db_path.exists() {
            let mut builder = fs::DirBuilder::new();
            builder.recursive(true);
            #[cfg(unix)]
            builder.mode(0o700);
            builder.create(db_path).map_err(LkpmError::Io)?;
        }
        #[cfg(unix)]
        {
            fs::set_permissions(db_path, fs::Permissions::from_mode(0o700))
            .map_err(LkpmError::Io)?;
        }
        Ok(())
    }
    #[cfg(unix)]
    fn ensure_db_file_mode(db_file: &PathBuf) -> Result<(), LkpmError> {
        fs::set_permissions(db_file, fs::Permissions::from_mode(0o600)).map_err(LkpmError::Io)
    }
    #[cfg(not(unix))]
    fn ensure_db_file_mode(_db_file: &PathBuf) -> Result<(), LkpmError> {
        Ok(())
    }
    pub fn load(cfg: &Config) -> Result<Self, LkpmError> {
        Self::ensure_db_dir(&cfg.db_path)?;
        let db_file = cfg.db_path.join("lkpm-database.db");
        let conn = Connection::open(&db_file)
            .map_err(|e| LkpmError::Other(format!("lkpm-database.db open error: {}", e)))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS installed_packages (
                name TEXT PRIMARY KEY,
                version TEXT,
                source TEXT,
                source_kind TEXT,
                package_path TEXT,
                checksum TEXT,
                files TEXT,
                requires TEXT,
                optdepends TEXT,
                conflicts TEXT,
                provides TEXT,
                backups TEXT,
                backups_hashes TEXT
            );",
        )
        .map_err(|e| LkpmError::Other(format!("lkpm-database.db init error: {}", e)))?;
        Self::ensure_db_file_mode(&db_file)?;
        Ok(Database { db_file })
    }
    fn open_mirrors_cache(cfg: &Config) -> Result<Connection, LkpmError> {
        Self::ensure_db_dir(&cfg.db_path)?;
        let cache_file = cfg.db_path.join("mirrors-cache.db");
        let conn = Connection::open(&cache_file)
            .map_err(|e| LkpmError::Other(format!("mirrors-cache.db open error: {}", e)))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS repo_cache (
                id TEXT PRIMARY KEY,
                repo_url TEXT,
                index_json TEXT,
                fetched_at INTEGER
            );",
        )
        .map_err(|e| LkpmError::Other(format!("mirrors-cache.db init error: {}", e)))?;
        Self::ensure_db_file_mode(&cache_file)?;
        Ok(conn)
    }
    pub fn store_repo_index(cfg: &Config, index_url: &str, index_json: &str) -> Result<(), LkpmError> {
        let conn = Self::open_mirrors_cache(cfg)?;
        conn.execute(
            "INSERT OR REPLACE INTO repo_cache (id, repo_url, index_json, fetched_at) VALUES (?1, ?2, ?3, strftime('%s','now'))",
            params![index_url, index_url, index_json],
        )
        .map_err(|e| LkpmError::Other(format!("mirrors-cache.db insert error: {}", e)))?;
        Ok(())
    }
    pub fn read_repo_index(cfg: &Config, index_url: &str) -> Result<Option<String>, LkpmError> {
        let cache_file = cfg.db_path.join("mirrors-cache.db");
        if !cache_file.exists() {
            return Ok(None);
        }
        let conn = Self::open_mirrors_cache(cfg)?;   
        let mut stmt = conn
            .prepare("SELECT index_json FROM repo_cache WHERE id = ?1")
            .map_err(|e| LkpmError::Other(format!("mirrors-cache.db query prepare error: {}", e)))?; 
        let mut rows = stmt
            .query_map(params![index_url], |row| row.get(0))
            .map_err(|e| LkpmError::Other(format!("mirrors-cache.db query error: {}", e)))?;
        if let Some(r) = rows.next() {
            let s: String = r.map_err(|e| LkpmError::Other(format!("mirrors-cache.db row error: {}", e)))?;
            Ok(Some(s))
        } else {
            Ok(None)
        }
    }
    fn conn(&self) -> Result<Connection, LkpmError> {
        Connection::open(&self.db_file).map_err(|e| LkpmError::Other(format!("lkpm-database.db open error: {}", e)))
    }
    pub fn register(&mut self, _cfg: &Config, package: InstalledPackage) -> Result<(), LkpmError> {
        let conn = self.conn()?;
        let files_json = serde_json::to_string(&package.files)
            .map_err(|e| LkpmError::Other(format!("lkpm-database.db serialize files error: {}", e)))?;
        let requires_json = serde_json::to_string(&package.requires)
            .map_err(|e| LkpmError::Other(format!("lkpm-database.db serialize requires error: {}", e)))?;
        let optdepends_json = serde_json::to_string(&package.optdepends)
            .map_err(|e| LkpmError::Other(format!("lkpm-database.db serialize optdepends error: {}", e)))?;
        let conflicts_json = serde_json::to_string(&package.conflicts)
            .map_err(|e| LkpmError::Other(format!("lkpm-database.db serialize conflicts error: {}", e)))?;
        let provides_json = serde_json::to_string(&package.provides)
            .map_err(|e| LkpmError::Other(format!("lkpm-database.db serialize provides error: {}", e)))?;
        let backups_json = serde_json::to_string(&package.backups)
            .map_err(|e| LkpmError::Other(format!("lkpm-database.db serialize backups error: {}", e)))?;
        let backups_hashes_json = serde_json::to_string(&package.backups_hashes)
            .map_err(|e| LkpmError::Other(format!("lkpm-database.db serialize backups_hashes error: {}", e)))?;
        conn.execute(
            "INSERT OR REPLACE INTO installed_packages (name, version, source, source_kind, package_path, checksum, files, requires, optdepends, conflicts, provides, backups, backups_hashes)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                package.name,
                package.version,
                package.source,
                package.source_kind,
                package.package_path.to_string_lossy(),
                package.checksum,
                files_json,
                requires_json,
                optdepends_json,
                conflicts_json,
                provides_json,
                backups_json,
                backups_hashes_json,
            ],
        )
        .map_err(|e| LkpmError::Other(format!("lkpm-database.db insert error: {}", e)))?;
        Ok(())
    }
    pub fn remove(&mut self, _cfg: &Config, name: &str) -> Result<Option<InstalledPackage>, LkpmError> {
        if let Some(pkg) = self.get(name)? {
            let conn = self.conn()?;
            conn.execute("DELETE FROM installed_packages WHERE name = ?1", params![name])
                .map_err(|e| LkpmError::Other(format!("lkpm-database.db delete error: {}", e)))?;
            Ok(Some(pkg))
        } else {
            Ok(None)
        }
    }
    pub fn get(&self, name: &str) -> Result<Option<InstalledPackage>, LkpmError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare("SELECT name, version, source, source_kind, package_path, checksum, files, requires, optdepends, conflicts, provides, backups, backups_hashes FROM installed_packages WHERE name = ?1")
            .map_err(|e| LkpmError::Other(format!("lkpm-database.db query prepare error: {}", e)))?;
        let mut rows = stmt
            .query_map(params![name], row_to_installed_package)
            .map_err(|e| LkpmError::Other(format!("lkpm-database.db query error: {}", e)))?;
        if let Some(res) = rows.next() {
            let pkg = res.map_err(|e| LkpmError::Other(format!("lkpm-database.db row error: {}", e)))?;
            Ok(Some(pkg))
        } else {
            Ok(None)
        }
    }
    pub fn find(&self, name: &str) -> Result<Option<InstalledPackage>, LkpmError> {
        self.get(name)
    }
    pub fn list(&self) -> Result<Vec<InstalledPackage>, LkpmError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare("SELECT name, version, source, source_kind, package_path, checksum, files, requires, optdepends, conflicts, provides, backups, backups_hashes FROM installed_packages")
            .map_err(|e| LkpmError::Other(format!("lkpm-database.db query prepare error: {}", e)))?;
        let rows = stmt
            .query_map([], row_to_installed_package)
            .map_err(|e| LkpmError::Other(format!("lkpm-database.db query error: {}", e)))?;
        let mut results = Vec::new();
        for r in rows {
            results.push(r.map_err(|e| LkpmError::Other(format!("lkpm-database.db row error: {}", e)))?);
        }
        Ok(results)
    }
}

fn row_to_installed_package(row: &rusqlite::Row) -> rusqlite::Result<InstalledPackage> {
    let files_json: String = row.get(6)?;
    let requires_json: String = row.get(7)?;
    let optdepends_json: String = row.get(8)?;
    let conflicts_json: String = row.get(9)?;
    let provides_json: String = row.get(10)?;
    let backups_json: Option<String> = row.get(11)?;
    let backups_hashes_json: Option<String> = row.get(12)?;
    let files: Vec<String> = serde_json::from_str(&files_json).unwrap_or_default();
    let requires: Vec<String> = serde_json::from_str(&requires_json).unwrap_or_default();
    let optdepends: Vec<String> = serde_json::from_str(&optdepends_json).unwrap_or_default();
    let conflicts: Vec<String> = serde_json::from_str(&conflicts_json).unwrap_or_default();
    let provides: Vec<String> = serde_json::from_str(&provides_json).unwrap_or_default();
    let backups: Vec<String> = backups_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();
    let backups_hashes: HashMap<String, String> = backups_hashes_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();
    Ok(InstalledPackage {
        name: row.get(0)?,
        version: row.get(1)?,
        source: row.get(2)?,
        source_kind: row.get(3)?,
        package_path: PathBuf::from(row.get::<_, String>(4)?),
        checksum: row.get(5)?,
        files: files.into_iter().map(PathBuf::from).collect(),
        requires,
        optdepends,
        conflicts,
        provides,
        backups: backups.into_iter().map(PathBuf::from).collect(),
        backups_hashes,
    })
}
