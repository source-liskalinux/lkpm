use anyhow::{Context, Result};
use flate2::read::GzDecoder;
use indicatif::ProgressBar;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufReader, Read};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use tar::{Archive, EntryType};
use xz2::read::XzDecoder;
use zstd::Decoder;
use crate::config::Config;
use crate::repo;

#[derive(Debug, Clone)]
pub struct PackageMetadata {
    pub name: String,
    pub version: String,
    pub arch: String,
    pub depends: Vec<String>,
    pub optdepends: Vec<String>,
    pub conflicts: Vec<String>,
    pub provides: Vec<String>,
    pub backups: Vec<String>,
}

pub struct BackupsAwareInstall {
    pub installed_files: Vec<PathBuf>,
    pub backups_hashes: HashMap<String, String>,
    pub update_backups: Vec<PathBuf>,
}

struct TempDirGuard<'a>(&'a Path);

impl Drop for TempDirGuard<'_> {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(self.0);
    }
}

pub fn read_package_metadata(path: &Path) -> Result<PackageMetadata> {
    let reader = open_package_reader(path)?;
    let mut archive = Archive::new(reader);
    for entry in archive.entries()? {
        let mut entry = entry?;
        let entry_path = sanitize_entry_path(&entry.path()?)?;
        if entry_path == Path::new(".PKGINFO") {
            let mut text = String::new();
            entry.read_to_string(&mut text)?;
            return parse_pkginfo(&text);
        }
    }
    anyhow::bail!(".PKGINFO is missing on {}!", path.display())
}

pub fn read_package_install_script(path: &Path) -> Result<Option<String>> {
    let reader = open_package_reader(path)?;
    let mut archive = Archive::new(reader);
    for entry in archive.entries()? {
        let mut entry = entry?;
        let entry_path = sanitize_entry_path(&entry.path()?)?;
        if entry_path == Path::new(".INSTALL") {
            let mut text = String::new();
            entry.read_to_string(&mut text)?;
            return Ok(Some(text));
        }
    }
    Ok(None)
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

pub fn run_install_hook(
    cfg: &Config,
    package_name: &str,
    script: Option<&str>,
    function: &str,
    args: &[&str],
) -> Result<()> {
    let Some(script) = script else {
        return Ok(());
    };
    let install_root = &cfg.install_root;
    let use_chroot = install_root.as_path() != Path::new("/") && install_root.as_path() != Path::new("");
    let install_dir = cfg.install_script_dir();
    fs::create_dir_all(&install_dir)
        .with_context(|| format!("failed to create {}", install_dir.display()))?;
    let script_file_name = format!("{}-{}-{}.sh", package_name, function, std::process::id());
    let host_path = install_dir.join(&script_file_name);
    fs::write(&host_path, script)
        .with_context(|| format!("failed to write install script cache file {}", host_path.display()))?;
    let shell_ref = if use_chroot {
        match host_path.strip_prefix(install_root) {
            Ok(rel) => format!("/{}", rel.to_string_lossy()),
            Err(_) => host_path.to_string_lossy().to_string(),
        }
    } else {
        host_path.to_string_lossy().to_string()
    };
    let arg_str = args.iter().map(|a| shell_quote(a)).collect::<Vec<_>>().join(" ");
    let cmd = format!(
        "source {} >/dev/null 2>&1 && if declare -f {} >/dev/null 2>&1; then {} {}; fi",
        shell_quote(&shell_ref),
        function,
        function,
        arg_str
    );
    let status: Result<()> = if use_chroot {
        match crate::chroot::run_in_chroot(install_root, crate::chroot::Shell::Bash(cmd.clone())) {
            Ok(code) if code == 0 => Ok(()),
            Ok(code) => anyhow::bail!("install script hook {} exited with status code {}", function, code),
            Err(e) => Err(e).with_context(|| format!("failed to run install script hook {} inside chroot", function)),
        }
    } else {
        match Command::new("bash").current_dir(install_root).arg("-c").arg(&cmd).status() {
            Ok(s) if s.success() => Ok(()),
            Ok(s) => anyhow::bail!("install script hook {} exited with {}", function, s),
            Err(e) => Err(e).with_context(|| format!("failed to run install script hook {}", function)),
        }
    };
    let _ = fs::remove_file(&host_path);
    status
}

fn is_package_metadata_file(path: &Path) -> bool {
    matches!(
        path,
        p if p == Path::new(".PKGINFO")
            || p == Path::new(".MTREE")
            || p == Path::new(".INSTALL")
            || p == Path::new(".BUILDINFO")
    )
}

pub fn list_package_files(path: &Path) -> Result<Vec<PathBuf>> {
    let reader = open_package_reader(path)?;
    let mut archive = Archive::new(reader);
    let mut files = Vec::new();
    for entry in archive.entries()? {
        let entry = entry?;
        let entry_path = sanitize_entry_path(&entry.path()?)?;
        if is_package_metadata_file(&entry_path) {
            continue;
        }
        if entry.header().entry_type() == EntryType::Directory {
            continue;
        }
        files.push(entry_path);
    }
    Ok(files)
}

pub fn install_package(
    path: &Path,
    install_root: &Path,
    pb: Option<&ProgressBar>,
) -> Result<Vec<PathBuf>> {
    let update_backup_root = install_root.join("etc/lkpm.d/backup");
    let result = install_package_with_backups(path, install_root, &[], &HashMap::new(), &update_backup_root, pb)?;
    Ok(result.installed_files)
}

fn hash_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 8192];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

pub fn install_package_with_backups(
    package_path: &Path,
    install_root: &Path,
    backups: &[String],
    previous_hashes: &HashMap<String, String>,
    update_backup_root: &Path,
    pb: Option<&ProgressBar>,
) -> Result<BackupsAwareInstall> {
    let conf = Config::load();
    let tmp_base = Config::tmp_install_dir(&conf);
    let pid = std::process::id();
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let tmp_dir = tmp_base.join(format!("extract-{}-{}", pid, timestamp));
    fs::create_dir_all(&tmp_dir)?;
    let _guard = TempDirGuard(&tmp_dir);
    let tmp_path = tmp_dir.as_path();
    {
        let reader = open_package_reader(package_path)?;
        let mut archive = Archive::new(reader);
        archive.set_preserve_permissions(true);
        archive.unpack(tmp_path)?;
    }
    let backup_set: HashMap<String, String> = backups
        .iter()
        .map(|b| (b.trim_start_matches('/').replace('\\', "/"), b.clone()))
        .collect();
    let mut installed_files = Vec::new();
    let mut backups_hashes = HashMap::new();
    let mut update_backups = Vec::new();
    let mut modified_backups = Vec::new();
    let reader = open_package_reader(package_path)?;
    let mut archive = Archive::new(reader);
    for entry in archive.entries()? {
        let entry = entry?;
        let entry_path = sanitize_entry_path(&entry.path()?)?;
        if is_package_metadata_file(&entry_path) {
            continue;
        }
        let rel_str = entry_path.to_string_lossy().replace('\\', "/");
        let target_path = install_root.join(&entry_path);
        installed_files.push(target_path.clone());
        if backup_set.contains_key(&rel_str) && target_path.exists() && target_path.is_file() {
            let existing_hash = hash_file(&target_path).unwrap_or_default();
            let pristine_hash = previous_hashes.get(&rel_str);
            let modified = match pristine_hash {
                Some(ph) => &existing_hash != ph,
                None => true,
            };
            if modified {
                let stash_dest = update_backup_root.join(&entry_path);
                if let Some(parent) = stash_dest.parent() {
                    let _ = fs::create_dir_all(parent);
                }
                let _ = fs::copy(&target_path, &stash_dest);
                update_backups.push(stash_dest.clone());
                modified_backups.push((target_path.clone(), stash_dest));
            }
        }
        if let Some(pb) = pb {
            pb.inc(1);
        }
    }
    {
        let reader = open_package_reader(package_path)?;
        let mut archive = Archive::new(reader);
        archive.set_preserve_permissions(true);
        archive.unpack(install_root)?;
    }
    for (target_path, stash_dest) in modified_backups {
        let _ = fs::copy(&stash_dest, &target_path);
    }
    for backup_rel in backup_set.keys() {
        let target_path = install_root.join(backup_rel);
        if target_path.exists() && target_path.is_file() {
            if let Ok(new_hash) = hash_file(&target_path) {
                backups_hashes.insert(backup_rel.clone(), new_hash);
            }
        }
    }
    Ok(BackupsAwareInstall {
        installed_files,
        backups_hashes,
        update_backups,
    })
}

fn open_package_reader(path: &Path) -> Result<Box<dyn Read>> {
    let file =
        File::open(path).with_context(|| format!("Failed to open {}", path.display()))?;
    let buffered = BufReader::new(file);
    let path_str = path.to_string_lossy().to_lowercase();
    if path_str.ends_with(".lsk.tar.zst") || path_str.ends_with(".tar.zst") || path_str.ends_with(".pkg.tar.zst") {
        let decoder = Decoder::new(buffered)?;
        Ok(Box::new(decoder))
    } else if path_str.ends_with(".lsk.tar.xz") || path_str.ends_with(".tar.xz") || path_str.ends_with(".pkg.tar.xz") {
        let decoder = XzDecoder::new(buffered);
        Ok(Box::new(decoder))
    } else if path_str.ends_with(".lsk.tar.gz") || path_str.ends_with(".tar.gz") || path_str.ends_with(".pkg.tar.gz") {
        let decoder = GzDecoder::new(buffered);
        Ok(Box::new(decoder))
    } else if path_str.ends_with(".lsk.tar") || path_str.ends_with(".tar") || path_str.ends_with(".pkg.tar") {
        Ok(Box::new(buffered))
    } else {
        anyhow::bail!("Unsupported package format: {}", path.display());
    }
}

fn sanitize_entry_path(path: &Path) -> Result<PathBuf> {
    let mut cleaned = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(name) => cleaned.push(name),
            Component::CurDir => continue,
            Component::ParentDir => {
                anyhow::bail!("Package contains unsafe path: {}", path.display())
            }
            _ => continue,
        }
    }
    Ok(cleaned)
}

fn parse_pkginfo(text: &str) -> Result<PackageMetadata> {
    let mut name = String::new();
    let mut version = String::new();
    let mut arch = String::new();
    let mut depends = Vec::new();
    let mut optdepends = Vec::new();
    let mut conflicts = Vec::new();
    let mut provides = Vec::new();
    let mut backups = Vec::new();
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("pkgname =") {
            name = value.trim().to_string();
        } else if let Some(value) = line.strip_prefix("pkgver =") {
            version = value.trim().to_string();
        } else if let Some(value) = line.strip_prefix("arch =") {
            arch = value.trim().to_string();
        } else if let Some(value) = line
            .strip_prefix("depend =")
            .or_else(|| line.strip_prefix("depends ="))
        {
            let dep = normalize_dependency_name(value);
            if !dep.is_empty() {
                depends.push(repo::canonical_package_name(&dep).to_string());
            }
        } else if let Some(value) = line
            .strip_prefix("optdepend =")
            .or_else(|| line.strip_prefix("optdepends ="))
        {
            let dep = normalize_dependency_name(value);
            if !dep.is_empty() {
                optdepends.push(repo::canonical_package_name(&dep).to_string());
            }
        } else if let Some(value) = line
            .strip_prefix("provide =")
            .or_else(|| line.strip_prefix("provides ="))
        {
            let dep = normalize_dependency_name(value);
            if !dep.is_empty() {
                provides.push(repo::canonical_package_name(&dep).to_string());
            }
        } else if let Some(value) = line
            .strip_prefix("conflict =")
            .or_else(|| line.strip_prefix("conflicts ="))
        {
            let dep = normalize_dependency_name(value);
            if !dep.is_empty() {
                conflicts.push(dep);
            }
        } else if let Some(value) = line
            .strip_prefix("backup =")
            .or_else(|| line.strip_prefix("backups ="))
        {
            let entry = value.trim().trim_start_matches('/').to_string();
            if !entry.is_empty() {
                backups.push(entry);
            }
        }
    }
    if name.is_empty() {
        anyhow::bail!("Package metadata missing pkgname!");
    }
    if version.is_empty() {
        anyhow::bail!("Package metadata missing pkgver!");
    }
    Ok(PackageMetadata {
        name,
        version,
        arch,
        depends,
        optdepends,
        conflicts,
        provides,
        backups,
    })
}

pub fn package_buildinfo_sha256(path: &Path) -> Result<Option<String>> {
    let reader = open_package_reader(path)?;
    let mut archive = Archive::new(reader);
    for entry in archive.entries()? {
        let mut entry = entry?;
        let entry_path = sanitize_entry_path(&entry.path()?)?;
        if entry_path == Path::new(".BUILDINFO") {
            let mut text = String::new();
            entry.read_to_string(&mut text)?;
            return parse_buildinfo_sha256(&text);
        }
    }
    Ok(None)
}

fn parse_buildinfo_sha256(text: &str) -> Result<Option<String>> {
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("pkgbuild_sha256sum") {
            let value = rest.trim_start();
            let value = value
                .strip_prefix('=')
                .or_else(|| value.strip_prefix("= "))
                .unwrap_or(value)
                .trim();
            if value.is_empty() {
                continue;
            }
            let mut hash = value.split_whitespace().next().unwrap_or(value).trim();
            if hash.starts_with('[') && hash.ends_with(']') && hash.len() > 1 {
                hash = &hash[1..hash.len() - 1];
            }
            let hash = hash.trim();
            if !hash.is_empty() {
                return Ok(Some(hash.to_string()));
            }
        }
    }
    Ok(None)
}

pub fn normalize_dependency_name(value: &str) -> String {
    let without_description = value.split_once(':').map(|(dep, _)| dep).unwrap_or(value);
    let mut end = without_description.len();
    for operator in ["<=", ">=", "=", "<", ">"] {
        if let Some(index) = without_description.find(operator) {
            end = end.min(index);
        }
    }
    without_description[..end].trim().to_string()
}