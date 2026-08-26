use crate::cli::Command;
use crate::config::Config;
use crate::database::{Database, InstalledPackage};
use crate::downloader::download_packages_concurrently;
use crate::error::LkpmError;
use crate::error::LkpmError::PackageNotFound;
use crate::pkg_utils as pkg;
use crate::repo;
use crate::ui;
use indicatif::ProgressBar;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::env;
use std::path::{Component, Path, PathBuf};
use std::time::Instant;
use colored::Colorize;

fn ensure_storage(cfg: &Config) -> Result<(), LkpmError> {
    fs::create_dir_all(&cfg.cache_path).map_err(LkpmError::Io)?;
    fs::create_dir_all(&cfg.db_path).map_err(LkpmError::Io)?;
    Ok(())
}

fn require_root() -> Result<(), LkpmError> {
    if unsafe { libc::getuid() } != 0 {
        Err(LkpmError::Other("Operation not permitted (os error 1)!".to_string()))
    } else {
        Ok(())
    }
}

fn require_root_for_install_root(cfg: &Config) -> Result<(), LkpmError> {
    if cfg.install_root == Path::new("/") {
        require_root()
    } else {
        Ok(())
    }
}

fn file_sha256(path: &PathBuf) -> Result<String, LkpmError> {
    let mut file = fs::File::open(path).map_err(LkpmError::Io)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0; 8192];
    loop {
        let count = std::io::Read::read(&mut file, &mut buffer).map_err(LkpmError::Io)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    let result = hasher.finalize();
    Ok(hex::encode(result))
}

fn package_size(path: &Path) -> u64 {
    fs::metadata(path)
        .map(|metadata| metadata.len())
        .unwrap_or(0)
}

fn source_is_remote(source: &str) -> bool {
    source.starts_with("http://") || source.starts_with("https://")
}

fn remove_remote_source(source: &str, path: &Path) {
    if source_is_remote(source) && path.exists() {
        let _ = fs::remove_file(path);
    }
}

// Result of extracting a single package to disk, carrying everything needed
// to (a) register it in the db and (b) run its post_install or post_upgrade
// hook later, once every package in the batch has been extracted.
struct ExtractedPackage {
    metadata: pkg::PackageMetadata,
    installed_files: Vec<PathBuf>,
    backups_paths: Vec<PathBuf>,
    backups_hashes: HashMap<String, String>,
    install_script: Option<String>,
    previous_version: Option<String>,
    previous_record: Option<InstalledPackage>,
    preserved_as_new: Vec<PathBuf>,
    backup_dir: Option<PathBuf>,
}

// Phase 1: run pre_install or pre_upgrade, then extract the package's files
// to "install_root". Does NOT run post_install or post_upgrade, that happens
// in "run_post_install_hook" after every package in the batch is extracted
// so a post hook that needs to chroot in (e.g. during bootstrap) can rely
// on the full set of packages already being on disk.
fn extract_package(
    cfg: &Config,
    db: &Database,
    metadata: &pkg::PackageMetadata,
    path: &Path,
) -> Result<ExtractedPackage, LkpmError> {
    let previous = find_installed_package(db, &metadata.name)?;
    let previous_hashes = previous
        .as_ref()
        .map(|p| p.backups_hashes.clone())
        .unwrap_or_default();
    let install_script = pkg::read_package_install_script(path).map_err(LkpmError::from)?;
    if let Some(prev) = &previous {
        if let Err(e) = pkg::run_install_hook(
            cfg,
            &metadata.name,
            install_script.as_deref(),
            "pre_upgrade",
            &[metadata.version.as_str(), prev.version.as_str()],
        ) {
            return Err(LkpmError::Other(format!(
                "pre_upgrade hook failed for {}: {}",
                metadata.name, e
            )));
        }
    } else if let Err(e) = pkg::run_install_hook(
        cfg,
        &metadata.name,
        install_script.as_deref(),
        "pre_install",
        &[metadata.version.as_str()],
    ) {
        return Err(LkpmError::Other(format!(
            "pre_install hook failed for {}: {}",
            metadata.name, e
        )));
    }
    let backup_dir = match &previous {
        Some(prev) => Some(backup_previous_files(cfg, prev)?),
        None => None,
    };
    let result =
        pkg::install_package_with_backups(path, &cfg.install_root, &metadata.backups, &previous_hashes, None)
            .map_err(LkpmError::from)?;
    for new_file in result.preserved_as_new.iter() {
        ui::warning(&format!(
            "{} was modified locally! New package version saved to {}.",
            new_file.with_extension("").display(),
            new_file.display()
        ));
    }
    let backups_paths: Vec<PathBuf> = metadata
        .backups
        .iter()
        .map(|p| cfg.install_root.join(p))
        .collect();
    Ok(ExtractedPackage {
        metadata: metadata.clone(),
        installed_files: result.installed_files,
        backups_paths,
        backups_hashes: result.backups_hashes,
        install_script,
        previous_version: previous.as_ref().map(|p| p.version.clone()),
        previous_record: previous,
        preserved_as_new: result.preserved_as_new,
        backup_dir,
    })
}

// Phase 2: run post_install or post_upgrade for an already-extracted package.
// Call this only after every package in the batch has gone through
// "extract_package" (and ideally been registered in the db), so hooks that
// chroot in can find every file they depend on already in place.
fn run_post_install_hook(cfg: &Config, extracted: &ExtractedPackage) -> Option<String> {
    let metadata = &extracted.metadata;
    if let Some(prev_version) = &extracted.previous_version {
        if let Err(e) = pkg::run_install_hook(
            cfg,
            &metadata.name,
            extracted.install_script.as_deref(),
            "post_upgrade",
            &[metadata.version.as_str(), prev_version.as_str()],
        ) {
            ui::warning(&format!("post_upgrade hook failed for {}: {}", metadata.name, e));
            return Some(format!("post_upgrade hook failed: {}", e));
        }
    } else if let Err(e) = pkg::run_install_hook(
        cfg,
        &metadata.name,
        extracted.install_script.as_deref(),
        "post_install",
        &[metadata.version.as_str()],
    ) {
        ui::warning(&format!("post_install hook failed for {}: {}", metadata.name, e));
        return Some(format!("post_install hook failed: {}", e));
    }
    // Past this point the batch is no longer rolled back for this package
    // (rollback only applies while extraction is still in progress), so the
    // pre-upgrade backup has served its purpose and can be freed.
    if let Some(backup_root) = &extracted.backup_dir {
        if let Err(e) = fs::remove_dir_all(backup_root) {
            ui::warning(&format!(
                "Failed to clean up backup for {}: {}",
                metadata.name, e
            ));
        }
    }
    None
}

// Copy every file the previous version of a package owns into
// "cfg.pkg_backup_dir()/<pkg>-<old-version>/....". (mirroring their path
// relative to "install_root"), before the new version overwrites them.
// Lets a rolled-back upgrade restore exact previous content instead of
// just deleting the new files and leaving the package uninstalled.
fn backup_previous_files(cfg: &Config, previous: &InstalledPackage) -> Result<PathBuf, LkpmError> {
    let backup_root = cfg
        .pkg_backup_dir()
        .join(format!("{}-{}", previous.name, previous.version));
    // Clear out any stale backup left over from an earlier failed attempt
    // before writing a fresh one.
    if backup_root.exists() {
        fs::remove_dir_all(&backup_root).map_err(LkpmError::Io)?;
    }
    fs::create_dir_all(&backup_root).map_err(LkpmError::Io)?;
    for file in previous.files.iter() {
        let rel = file.strip_prefix(&cfg.install_root).unwrap_or(file);
        let dest = backup_root.join(rel);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(LkpmError::Io)?;
        }
        match fs::symlink_metadata(file) {
            Ok(meta) if meta.file_type().is_symlink() => {
                let target = fs::read_link(file).map_err(LkpmError::Io)?;
                std::os::unix::fs::symlink(&target, &dest).map_err(LkpmError::Io)?;
            }
            Ok(meta) if meta.file_type().is_file() => {
                fs::copy(file, &dest).map_err(LkpmError::Io)?;
            }
            // Missing or a special file (device / fifo / socket), nothing
            // sensible to snapshot so skip it. A rollback simply won't
            // recreate it either.
            _ => {}
        }
    }
    Ok(backup_root)
}

// Copy every file backed up by "backup_previous_files" back to its
// original location under "install_root".
fn restore_backed_up_files(cfg: &Config, backup_root: &Path, previous: &InstalledPackage) {
    for file in previous.files.iter() {
        let rel = file.strip_prefix(&cfg.install_root).unwrap_or(file);
        let src = backup_root.join(rel);
        let meta = match fs::symlink_metadata(&src) {
            Ok(meta) => meta,
            Err(_) => continue,
        };
        if let Some(parent) = file.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if meta.file_type().is_symlink() {
            if let Ok(target) = fs::read_link(&src) {
                let _ = fs::remove_file(file);
                let _ = std::os::unix::fs::symlink(&target, file);
            }
        } else if meta.file_type().is_file() {
            if let Err(e) = fs::copy(&src, file) {
                ui::warning(&format!(
                    "Failed to restore {} from backup while rolling back {}: {}",
                    file.display(), previous.name, e
                ));
            }
        }
    }
}

fn rollback_extracted_package(cfg: &Config, db: &mut Database, extracted: &ExtractedPackage) {
    ui::warning(&format!(
        "Rolling back {} ({})....",
        extracted.metadata.name, extracted.metadata.version
    ));
    for file in extracted.preserved_as_new.iter() {
        if file.exists() {
            let _ = fs::remove_file(file);
        }
    }
    match (&extracted.backup_dir, &extracted.previous_record) {
        (Some(backup_root), Some(prev)) => {
            // Upgrade: restore every backed-up file to its previous content.
            restore_backed_up_files(cfg, backup_root, prev);
            // Remove any file this upgrade introduced that the old version
            // didn't have, it has nothing to be restored to.
            let prev_files: HashSet<&PathBuf> = prev.files.iter().collect();
            for file in extracted.installed_files.iter() {
                if !prev_files.contains(file) && file.exists() {
                    if let Err(e) = fs::remove_file(file) {
                        ui::warning(&format!(
                            "Failed to remove new file {} while rolling back {}: {}",
                            file.display(), extracted.metadata.name, e
                        ));
                    }
                }
            }
            if let Err(e) = db.register(cfg, prev.clone()) {
                ui::warning(&format!(
                    "Failed to restore {} to {} in the package database: {}",
                    extracted.metadata.name, prev.version, e
                ));
            }
            if let Err(e) = fs::remove_dir_all(backup_root) {
                ui::warning(&format!(
                    "Failed to clean up backup for {}: {}",
                    extracted.metadata.name, e
                ));
            }
            ui::warning(&format!(
                "{} restored to {}.",
                extracted.metadata.name, prev.version
            ));
        }
        (None, Some(prev)) => {
            // Shouldn't normally happen (extract_package always backs up
            // when there's a previous version), fall back to best-effort:
            // remove the new files, but content can't be restored.
            for file in extracted.installed_files.iter() {
                if file.exists() {
                    let _ = fs::remove_file(file);
                }
            }
            if let Err(e) = db.remove(cfg, &extracted.metadata.name) {
                ui::warning(&format!(
                    "Failed to remove {} from the package database during rollback: {}",
                    extracted.metadata.name, e
                ));
            }
            ui::warning(&format!(
                "{} was upgrading from {} but no backup was found! Rollback removed its new files but it could not be restored to {}!",
                extracted.metadata.name, prev.version, prev.version
            ));
        }
        (_, None) => {
            // Fresh install: nothing existed before, so fully undo it.
            for file in extracted.installed_files.iter() {
                if file.exists() {
                    if let Err(e) = fs::remove_file(file) {
                        ui::warning(&format!(
                            "Failed to remove {} while rolling back {}: {}",
                            file.display(), extracted.metadata.name, e
                        ));
                    }
                }
            }
            if let Err(e) = db.remove(cfg, &extracted.metadata.name) {
                ui::warning(&format!(
                    "Failed to remove {} from the package database during rollback: {}",
                    extracted.metadata.name, e
                ));
            }
        }
    }
}

fn package_list_contains(list: &[String], package: &str) -> bool {
    let lower_pkg = package.to_ascii_lowercase();
    let lower_canonical = repo::canonical_package_name(package).to_ascii_lowercase();
    list.iter().any(|entry| {
        let lower_entry = entry.to_ascii_lowercase();
        if lower_entry.ends_with('*') {
            let prefix = lower_entry.trim_end_matches('*');
            if prefix.is_empty() {
                return true;
            }
            lower_pkg.starts_with(prefix) || lower_canonical.starts_with(prefix)
        } else {
            lower_entry == lower_pkg || lower_entry == lower_canonical
        }
    })
}

fn canonical_package_name(package: &str) -> String {
    repo::canonical_package_name(package).to_string()
}

fn find_installed_package(db: &Database, package: &str) -> Result<Option<InstalledPackage>, LkpmError> {
    let clean_package = pkg::normalize_dependency_name(package);
    if let Some(installed) = db.find(&clean_package)? {
        return Ok(Some(installed));
    }
    let canonical = canonical_package_name(&clean_package);
    if canonical != clean_package {
        if let Some(installed) = db.find(&canonical)? {
            return Ok(Some(installed));
        }
    }
    for installed in db.list()?.into_iter() {
        if installed
            .provides
            .iter()
            .any(|provide| package_list_contains(&[pkg::normalize_dependency_name(provide)], &clean_package))
        {
            return Ok(Some(installed));
        }
        if canonical != clean_package
            && installed
                .provides
                .iter()
                .any(|provide| package_list_contains(&[pkg::normalize_dependency_name(provide)], &canonical))
        {
            return Ok(Some(installed));
        }
    }
    Ok(None)
}

struct PreparedInstall {
    target: InstallTarget,
    path: PathBuf,
    metadata: pkg::PackageMetadata,
    checksum: String,
}

const BOOTSTRAP_ESSENTIALS: &[&str] = &[
    "filesystem", "iana-etc", "glibc", "busybox", "bash", "coreutils", "util-linux", "kmod",
];

fn is_bootstrap_essential(name: &str) -> bool {
    BOOTSTRAP_ESSENTIALS.contains(&name)
}

fn pick_next(ready: &mut Vec<String>) -> Option<String> {
    let idx = ready
        .iter()
        .enumerate()
        .min_by_key(|(_, name)| (!is_bootstrap_essential(name), name.as_str()))
        .map(|(i, _)| i)?;
    Some(ready.remove(idx))
}

fn topological_install_order_from_planned(
    planned: &HashMap<String, PlannedInstall>,
) -> Result<Vec<String>, LkpmError> {
    let mut in_degree: HashMap<String, usize> = HashMap::new();
    let mut graph: HashMap<String, Vec<String>> = HashMap::new();
    for node in planned.keys() {
        in_degree.entry(node.clone()).or_insert(0);
        graph.entry(node.clone()).or_default();
    }
    for (pkg_name, item) in planned {
        for dep in &item.depends {
            let clean_dep = pkg::normalize_dependency_name(dep);
            let dep_canonical = canonical_package_name(&clean_dep);
            if dep_canonical == *pkg_name {
                continue;
            }
            if planned.contains_key(&dep_canonical) {
                graph.entry(dep_canonical.to_string()).or_default().push(pkg_name.clone());
                *in_degree.entry(pkg_name.clone()).or_insert(0) += 1;
            }
        }
    }
    let mut ready: Vec<String> = in_degree
        .iter()
        .filter(|&(_, &deg)| deg == 0)
        .map(|(node, _)| node.clone())
        .collect();
    let mut order = Vec::new();
    while order.len() < planned.len() {
        if let Some(node) = pick_next(&mut ready) {
            order.push(node.clone());
            if let Some(neighbors) = graph.get(&node) {
                for neighbor in neighbors {
                    if let Some(deg) = in_degree.get_mut(neighbor) {
                        if *deg > 0 {
                            *deg -= 1;
                            if *deg == 0 {
                                ready.push(neighbor.clone());
                            }
                        }
                    }
                }
            }
        } else {
            let next_node = in_degree
                .iter()
                .filter(|(node, _)| !order.contains(node))
                .min_by_key(|&(node, &deg)| (deg, node.clone()))
                .map(|(node, _)| node.clone());
            if let Some(node) = next_node {
                in_degree.insert(node.clone(), 0);
                ready.push(node);
            } else {
                break;
            }
        }
    }
    Ok(order)
}

struct PlannedInstall {
    target: InstallTarget,
    depends: Vec<String>,
}

fn resolve_installation_plan(
    cfg: &Config,
    db: &Database,
    initial_targets: Vec<InstallTarget>,
) -> Result<Vec<InstallTarget>, LkpmError> {
    ui::info(&format!("Resolving package dependencies for {} package(s)....", initial_targets.len()));
    let mut planned: HashMap<String, PlannedInstall> = HashMap::new();
    let mut queue: VecDeque<InstallTarget> = VecDeque::new();
    let mut visited: HashSet<String> = HashSet::new();
    for target in initial_targets {
        queue.push_back(target);
    }
    while let Some(target) = queue.pop_front() {
        let requested_name = target
            .requested_name
            .as_deref()
            .unwrap_or_else(|| target.source.as_str());
        let package_name = canonical_package_name(&pkg::normalize_dependency_name(requested_name));
        if let Some(installed) = find_installed_package(db, &package_name)? {
            if !visited.insert(installed.name.clone()) {
                continue;
            }
            continue;
        }
        if planned.contains_key(&package_name) {
            continue;
        }
        let repo_info = match repo::find_repo_package_info(cfg, &package_name)? {
            Some(info) => info,
            None => {
                continue;
            }
        };
        let actual_name = canonical_package_name(&repo_info.name);
        if planned.contains_key(&actual_name) {
            continue;
        }
        for dep in repo_info.depends.iter() {
            let clean_dep = pkg::normalize_dependency_name(dep);
            let dep_name = canonical_package_name(&clean_dep);
            if find_installed_package(db, &dep_name)?.is_some() {
                continue;
            }
            if planned.contains_key(&dep_name) || visited.contains(&dep_name) {
                continue;
            }
            if let Some(repo_location) = repo::find_pkg_in_repos_location(cfg, &dep_name)? {
                queue.push_back(InstallTarget {
                    requested_name: Some(dep_name.clone()),
                    source: match &repo_location {
                        repo::PackageLocation::Remote(url) => url.clone(),
                    },
                    location: repo_location,
                    metadata: None,
                });
            } else {
                continue;
            }
        }
        planned.insert(
            actual_name.clone(),
            PlannedInstall {
                target,
                depends: repo_info.depends.iter().map(|d| pkg::normalize_dependency_name(d)).collect(),
            },
        );
        visited.insert(actual_name);
    }
    let order = topological_install_order_from_planned(&planned)?;
    let ordered_targets = order
        .into_iter()
        .filter_map(|pkg_name| planned.remove(&pkg_name).map(|p| p.target))
        .collect();
    Ok(ordered_targets)
}

fn is_blocked_arch_package(cfg: &Config, package: &str) -> bool {
    package_list_contains(&cfg.blocked_packages, package)
}

fn ensure_package_is_not_blocked(cfg: &Config, package: &str) -> Result<(), LkpmError> {
    if is_blocked_arch_package(cfg, package) {
        return Err(PackageNotFound(package.to_string()));
    }
    Ok(())
}

fn check_repository_connections(cfg: &Config) -> Result<(), LkpmError> {
    ui::info("Checking repository connections....");
    let core_urls = cfg.core_repos.iter().map(|r| r.to_string()).collect::<Vec<_>>();
    let extra_urls = cfg
        .extra_mirrors
        .iter()
        .map(|r| format!("{}/", r.trim_end_matches('/')))
        .collect::<Vec<_>>();
    let mut connected_core = 0usize;
    let mut connected_extra = 0usize;
    for repo_url in core_urls.iter() {
        match repo::repo_is_reachable(repo_url) {
            Ok(true) => {
                connected_core += 1;
                ui::success(&format!("Connected: {}", repo_url));
            }
            Ok(false) => {
                ui::error(&format!("Failed to connect {}!", repo_url));
            }
            Err(err) => {
                ui::error(&format!("Failed to connect {}! Error message: {}", repo_url, err));
            }
        }
    }
    for repo_url in extra_urls.iter() {
        match repo::repo_is_reachable(repo_url) {
            Ok(true) => {
                connected_extra += 1;
                ui::success(&format!("Connected: {}", repo_url));
            }
            Ok(false) => {
                ui::error(&format!("Failed to connect {}!", repo_url));
            }
            Err(err) => {
                ui::error(&format!("Failed to connect {}! Error message: {}", repo_url, err));
            }
        }
    }
    if connected_core == 0 && !core_urls.is_empty() {
        return Err(LkpmError::Network("No core repositories are reachable at this time!".into()));
    }
    let total_repos = core_urls.len() + extra_urls.len();
    let total_connected = connected_core + connected_extra;
    if total_repos > 0 {
        ui::success(&format!(
            "Repository check completed: ({}/{}) repositories reachable!",
            total_connected,
            total_repos
        ));
    }
    Ok(())
}

#[derive(Clone)]
struct InstallTarget {
    requested_name: Option<String>,
    source: String,
    location: repo::PackageLocation,
    metadata: Option<pkg::PackageMetadata>,
}

fn build_install_target(cfg: &Config, package: &str) -> Result<InstallTarget, LkpmError> {
    if package.starts_with("http://") || package.starts_with("https://") {
        return Ok(InstallTarget {
            requested_name: None,
            source: package.to_string(),
            location: repo::PackageLocation::Remote(package.to_string()),
            metadata: None,
        });
    }
    ensure_package_is_not_blocked(cfg, package)?;
    let repo_info = repo::find_repo_package_info(cfg, package)?
        .ok_or_else(|| LkpmError::PackageNotFound(format!("{} not found in repositories!", package)))?;
    let location = repo::find_pkg_in_repos_location(cfg, &repo_info.name)?
        .ok_or_else(|| LkpmError::PackageNotFound(format!("Download location for {} not found!", package)))?;
    Ok(InstallTarget {
        requested_name: Some(repo_info.name.clone()),
        source: match &location {
            repo::PackageLocation::Remote(url) => url.clone(),
        },
        location,
        metadata: None,
    })
}

fn validate_package_metadata(
    cfg: &Config,
    metadata: &pkg::PackageMetadata,
) -> Result<(), LkpmError> {
    ensure_package_is_not_blocked(cfg, &metadata.name)?;
    if !metadata.arch.is_empty() && metadata.arch != "any" && metadata.arch != cfg.arch.as_str() {
        return Err(LkpmError::Other(format!(
            "Package architecture mismatch: expected {} but found {}.",
            cfg.arch, metadata.arch
        )));
    }
    Ok(())
}

fn confirm_operation(noconfirm: bool, prompt: &str, default: bool) -> bool {
    if noconfirm {
        true
    } else {
        ui::confirm(prompt, default)
    }
}

fn apply_root_override(cfg: &mut Config, root: Option<PathBuf>) -> Result<(), LkpmError> {
    if let Some(root) = root {
        if root.as_os_str().is_empty() {
            return Err(LkpmError::Other("Install root cannot be empty!".into()));
        }
        cfg.install_root = root.clone();
        cfg.db_path = root.join("var/db/lkpm");
        cfg.cache_path = root.join("var/cache/lkpm");
        cfg.apply_system_config_for_root(&root);
        cfg.reload_mirrorlist_for_root();
    }
    Ok(())
}

fn original_user_home() -> Option<PathBuf> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| Some(PathBuf::from("/root")))
        .or_else(|| Some(PathBuf::from("/")))
}

fn resolve_local_package_path(raw: &str) -> PathBuf {
    let path = PathBuf::from(raw);
    if path.exists() {
        return path;
    }
    if let Some(home) = original_user_home() {
        let candidate = home.join(raw);
        if candidate.exists() {
            return candidate;
        }
    }
    path
}

fn install_local_packages(
    cfg: &Config,
    db: &mut Database,
    packages: Vec<String>,
    install_deps: bool,
    noconfirm: bool,
) -> Result<(), LkpmError> {
    if packages.is_empty() {
        return Err(LkpmError::Other(
            "No local package file(s) specified for installation!".into(),
        ));
    }
    require_root_for_install_root(cfg)?;
    ui::start_operation("Starting local installation....");
    let duration = Instant::now();
    let mut targets = Vec::new();
    for raw in packages.iter() {
        let path = resolve_local_package_path(raw);
        if !path.exists() || !path.is_file() {
            return Err(LkpmError::Other(format!(
                "Local package file {} was not found!",
                raw
            )));
        }
        let metadata = pkg::read_package_metadata(&path).map_err(LkpmError::from)?;
        validate_package_metadata(cfg, &metadata)?;
        targets.push((path, metadata));
    }
    ui::info(&format!("About to install {} local package(s):", targets.len()));
    for (path, metadata) in targets.iter() {
        println!(
            "    ‣ {} ({}) [{}]",
            metadata.name.bright_yellow(),
            metadata.version.bright_green(),
            path.display()
        );
    }
    let mut missing_deps: Vec<String> = Vec::new();
    if install_deps {
        for (_, metadata) in targets.iter() {
            for dep in metadata.depends.iter() {
                let dep_name = canonical_package_name(&pkg::normalize_dependency_name(dep));
                if find_installed_package(db, &dep_name)?.is_none() && !missing_deps.contains(&dep_name) {
                    missing_deps.push(dep_name);
                }
            }
        }
        if !missing_deps.is_empty() {
            let deps: Vec<(&str, bool)> = missing_deps.iter().map(|d| (d.as_str(), false)).collect();
            ui::dependency_report(&deps, &[]);
        }
    }
    let default = true;
    if !confirm_operation(noconfirm, "Proceed to install?", default) {
        ui::error("Installation aborted by the user.");
        return Ok(());
    }
    if !missing_deps.is_empty() {
        ui::info(&format!(
            "{} {} {}",
            "Resolving".bright_cyan(),
            missing_deps.len(),
            "missing dependencies from configured repositories....".bright_cyan()
        ));
        fs::create_dir_all(&cfg.cache_path).map_err(LkpmError::Io)?;
        let _ = repo::refresh_repo_metadata(cfg)?;
        handle(Command::Install {
            packages: missing_deps,
            install_deps: true,
            local: false,
            noconfirm: true,
            root: None,
        })?;
        *db = Database::load(cfg)?;
    }
    // Phase 1: extract every package first (pre_install or pre_upgrade plus
    // file extraction). Post_install or post_upgrade hooks are deferred to phase 
    // 2 below, run only once all packages are on disk, this is important for
    // bootstrap installs where a post_install hook needs to chroot in and
    // relies on other packages (e.g. busybox or bash) already being extracted.
    struct PendingLocal {
        path: PathBuf,
        checksum: String,
        size: u64,
        extracted: ExtractedPackage,
    }
    let mut pending: Vec<PendingLocal> = Vec::new();
    let mut results: Vec<ui::PackageSummary> = Vec::new();
    ui::info(&format!("{} {} {}", "Installing".bright_cyan(), targets.len(), "package(s)....".bright_cyan()));
    for (path, metadata) in targets.into_iter() {
        ui::info(&format!(
            "➔ Installing {} ({}) from {}....",
            metadata.name.bright_yellow(),
            metadata.version.bright_green(),
            path.display()
        ));
        let checksum = match file_sha256(&path) {
            Ok(sha) => sha,
            Err(err) => {
                ui::warning(&format!("Failed to calculate checksum for {}: {}", metadata.name, err));
                String::new()
            }
        };
        let size = package_size(&path);
        match extract_package(cfg, db, &metadata, &path) {
            Ok(extracted) => {
                // Register in the db right away so later packages in this
                // same batch resolve it as installed (e.g. dependents), even
                // though its post_install hook hasn't run yet.
                db.register(
                    cfg,
                    InstalledPackage {
                        name: extracted.metadata.name.clone(),
                        version: extracted.metadata.version.clone(),
                        source: path.to_string_lossy().to_string(),
                        source_kind: "local".into(),
                        package_path: path.clone(),
                        checksum: checksum.clone(),
                        files: extracted.installed_files.clone(),
                        requires: extracted.metadata.depends.clone(),
                        optdepends: extracted.metadata.optdepends.clone(),
                        conflicts: extracted.metadata.conflicts.clone(),
                        provides: extracted.metadata.provides.clone(),
                        backups: extracted.backups_paths.clone(),
                        backups_hashes: extracted.backups_hashes.clone(),
                        install_script: extracted.install_script.clone(),
                    },
                )?;
                pending.push(PendingLocal { path, checksum, size, extracted });
            }
            Err(err) => {
                for item in pending.into_iter().rev() {
                    rollback_extracted_package(cfg, db, &item.extracted);
                }
                return Err(LkpmError::Other(format!(
                    "Failed to extract {} ({}): {}. Aborting the whole installation! No post_install hooks were run for any package in this batch.",
                    metadata.name, metadata.version, err
                )));
            }
        }
    }
    // Phase 2: every package that extracted cleanly now has its files on
    // disk, so it's safe to run post_install or post_upgrade hooks in order.
    ui::info(&format!("{} {} {}", "Running post-install hooks for".bright_cyan(), pending.len(), "package(s)....".bright_cyan()));
    for item in pending.into_iter() {
        let hook_failure = run_post_install_hook(cfg, &item.extracted);
        let status = match hook_failure {
            Some(reason) => format!("installed (warning: {})", reason),
            None => "installed".to_string(),
        };
        results.push(ui::PackageSummary {
            name: item.extracted.metadata.name.clone(),
            version: item.extracted.metadata.version.clone(),
            source: item.path.display().to_string(),
            size: item.size,
            duration: duration.elapsed(),
            checksum: item.checksum,
            status,
        });
    }
    ui::print_operation_summary(&results);
    Ok(())
}

pub fn handle(cmd: Command) -> Result<(), LkpmError> {
    let mut cfg = Config::load();
    match &cmd {
        Command::Install { root, .. }
        | Command::Update { root, .. }
        | Command::Delete { root, .. }
        | Command::Refresh { root }
        | Command::UpdateRefresh { root, .. } => apply_root_override(&mut cfg, root.clone())?,
        Command::Package { root, .. } => apply_root_override(&mut cfg, root.clone())?,
        _ => {}
    }
    ensure_storage(&cfg)?;
    let mut db = Database::load(&cfg)?;
    match cmd {
        Command::Install {
            packages,
            install_deps,
            local,
            noconfirm,
            ..
        } if local => install_local_packages(&cfg, &mut db, packages, install_deps, noconfirm),
        Command::Install {
            packages,
            install_deps,
            noconfirm,
            ..
        } => {
            if packages.is_empty() {
                return Err(LkpmError::Other(
                    "No packages specified for installation!".into(),
                ));
            }
            require_root_for_install_root(&cfg)?;
            ui::start_operation("Starting the operation....");
            let duration = Instant::now();
            ui::info("Refreshing repository metadata....");
            fs::create_dir_all(&cfg.cache_path).map_err(LkpmError::Io)?;
            let refreshed = repo::refresh_repo_metadata(&cfg)?;
            if refreshed.is_empty() {
                if cfg.core_repos.is_empty() && cfg.extra_mirrors.is_empty() {
                    ui::error("No main repository that has been configured!");
                } else {
                    ui::warning("Repository metadata refresh completed but no metadata files were refreshed!");
                }
            } else {
                ui::success("Repository metadata has been refreshed successfully!");
            }
            check_repository_connections(&cfg)?;
            if refreshed.is_empty() {
                ui::warning("Repository host is reachable but metadata cache could not be refreshed!");
            } else {
                ui::success("Repository metadata cache is ready.");
            }
            let mut installs = Vec::new();
            for pkg_name in packages.iter() {
                let target = build_install_target(&cfg, pkg_name)?;
                installs.push(target);
            }
            let total_installs = installs.len();
            let mut dep_map = std::collections::BTreeMap::new();
            let mut all_optional: Vec<&str> = Vec::new();
            for target in installs.iter() {
                if let Some(metadata) = target.metadata.as_ref() {
                    for dep in metadata.depends.iter() {
                        let installed = find_installed_package(&db, dep)?.is_some();
                        dep_map.insert(dep.clone(), installed);
                    }
                    for opt in metadata.optdepends.iter() {
                        all_optional.push(opt);
                    }
                }
            }
            let deps: Vec<(&str, bool)> = dep_map
                .iter()
                .map(|(dep, installed)| (dep.as_str(), *installed))
                .collect();
            if !deps.is_empty() || !all_optional.is_empty() {
                ui::dependency_report(&deps, &all_optional);
            }
            let initial_targets: Vec<InstallTarget> = installs.into_iter().collect();
            let mut results: Vec<ui::PackageSummary> = Vec::new();
            let default = true;
            let plan_targets = if install_deps {
                let resolved = resolve_installation_plan(&cfg, &db, initial_targets)?;
                ui::info(&format!("About to install {} package(s):", resolved.len()));
                for target in resolved.iter() {
                    let pkg_name = target.requested_name.as_deref().unwrap_or(&target.source);
                    println!("    ‣ {}", pkg_name.bright_yellow());
                }
                if !confirm_operation(noconfirm, "Proceed to install?", default) {
                    ui::error("Installation aborted by the user.");
                    return Ok(());
                }
                resolved
            } else {
                ui::info(&format!("About to install {} package(s):", total_installs));
                for install in packages.iter() {
                    println!("    ‣ {}", install.bright_yellow());
                }
                if !confirm_operation(noconfirm, "Proceed to install?", default) {
                    ui::error("Installation aborted by the user.");
                    return Ok(());
                }
                initial_targets
            };
            ui::info(&format!(
                "{} {} {}",
                "Downloading".bright_cyan(),
                plan_targets.len(),
                "package(s)....".bright_cyan()
            ));
            let urls: Vec<String> = plan_targets
                .iter()
                .map(|t| match &t.location {
                    repo::PackageLocation::Remote(u) => u.clone(),
                })
                .collect();
            let downloaded_paths = download_packages_concurrently(&cfg, &urls)?;
            let mut prepared = Vec::new();
            for (i, target) in plan_targets.into_iter().enumerate() {
                if let Some(path) = &downloaded_paths[i] {
                    let path = path.clone();
                    let pkg_name = target.requested_name.as_deref().unwrap_or(&target.source);
                    let checksum = match file_sha256(&path) {
                        Ok(sha) => sha,
                        Err(err) => {
                            ui::warning(&format!("Failed to calculate checksum for {}: {}", pkg_name, err));
                            continue;
                        }
                    };
                    let metadata = pkg::read_package_metadata(&path)?;
                    validate_package_metadata(&cfg, &metadata)?;
                    prepared.push(PreparedInstall { target, path, metadata, checksum });
                } else {
                    continue;
                }
            }
            ui::info(&format!(
                "{} {} {}",
                "Installing".bright_cyan(),
                prepared.len(),
                "package(s) to system:".bright_cyan()
            ));
            // Phase 1: extract every downloaded package first, deferring
            // post_install or post_upgrade hooks to phase 2 below so they only
            // run once the whole batch is on disk (needed for bootstrap
            // installs where a hook chroots in and expects e.g. busybox or bash
            // to already be extracted).
            struct PendingRemote {
                target: InstallTarget,
                path: PathBuf,
                checksum: String,
                extracted: ExtractedPackage,
            }
            let mut pending: Vec<PendingRemote> = Vec::new();
            for prepared in prepared.into_iter() {
                let target = prepared.target.clone();
                let path = prepared.path.clone();
                let metadata = prepared.metadata.clone();
                let checksum = prepared.checksum.clone();
                ui::info(&format!(
                    "➔ Installing {} ({})....",
                    metadata.name.bright_yellow(),
                    metadata.version.bright_green()
                ));
                match extract_package(&cfg, &db, &metadata, &path) {
                    Ok(extracted) => {
                        let package_path = path.clone();
                        db.register(
                            &cfg,
                            InstalledPackage {
                                name: extracted.metadata.name.clone(),
                                version: extracted.metadata.version.clone(),
                                source: target.source.clone(),
                                source_kind: "remote".into(),
                                package_path,
                                checksum: checksum.clone(),
                                files: extracted.installed_files.clone(),
                                requires: extracted.metadata.depends.clone(),
                                optdepends: extracted.metadata.optdepends.clone(),
                                conflicts: extracted.metadata.conflicts.clone(),
                                provides: extracted.metadata.provides.clone(),
                                backups: extracted.backups_paths.clone(),
                                backups_hashes: extracted.backups_hashes.clone(),
                                install_script: extracted.install_script.clone(),
                            },
                        )?;
                        pending.push(PendingRemote { target, path, checksum, extracted });
                    }
                    Err(err) => {
                        for item in pending.into_iter().rev() {
                            rollback_extracted_package(&cfg, &mut db, &item.extracted);
                        }
                        crate::downloader::clear_download_dir(&cfg);
                        return Err(LkpmError::Other(format!(
                            "Failed to extract {} ({}): {}. Aborting the whole installation! No post_install hooks were run for any package in this batch.",
                            metadata.name, metadata.version, err
                        )));
                    }
                }
            }
            // Phase 2: run post_install or post_upgrade now that every package
            // in this batch has its files in place.
            ui::info(&format!(
                "{} {} {}",
                "Running post-install hooks for".bright_cyan(),
                pending.len(),
                "package(s)....".bright_cyan()
            ));
            for item in pending.into_iter() {
                let size = package_size(&item.path);
                let hook_failure = run_post_install_hook(&cfg, &item.extracted);
                remove_remote_source(&item.target.source, &item.path);
                let status = match hook_failure {
                    Some(reason) => format!("installed (warning: {})", reason),
                    None => "installed".to_string(),
                };
                results.push(ui::PackageSummary {
                    name: item.extracted.metadata.name.clone(),
                    version: item.extracted.metadata.version.clone(),
                    source: repo::repo_source_label(&cfg, &item.target.source),
                    size,
                    duration: duration.elapsed(),
                    checksum: item.checksum,
                    status,
                });
            }
            ui::print_operation_summary(&results);
            Ok(())
        }
        Command::Delete {
            packages,
            noconfirm,
            ..
        } => {
            if packages.is_empty() {
                return Err(LkpmError::Other(
                    "No packages specified for deletion!".into(),
                ));
            }
            require_root_for_install_root(&cfg)?;
            ui::start_operation("Starting the operation....");
            let duration = Instant::now();
            let mut removals = Vec::new();
            for pkg_name in packages.iter() {
                if let Some(record) = find_installed_package(&db, pkg_name)? {
                    removals.push(record);
                } else {
                    return Err(LkpmError::Other(format!("{} is not installed on the system!", pkg_name)));
                }
            }
            let mut dependents = Vec::new();
            for removal in removals.iter() {
                for installed in db.list()?.iter() {
                        if installed.name != removal.name
                            && installed.requires.iter().any(|r| r == &removal.name)
                        {
                            dependents.push(installed.name.clone());
                        }
                    }
            }
            dependents.sort();
            dependents.dedup();
            if !dependents.is_empty() {
                ui::reverse_dependency_report(&dependents);
            }
            ui::info(&format!("About to delete {} package(s):", removals.len()));
            for removal in packages.iter() {
                println!(
                    "    ‣ {}",
                    removal.bright_yellow()
                );
            }
            let default = false;
            if !confirm_operation(noconfirm, "Proceed to delete?", default) {
                ui::error("Deletion aborted by the user.");
                return Ok(());
            }
            ui::info(&format!("{} {} {}", "Deleting".bright_cyan(), removals.len(), "package(s):".bright_cyan()));
            let mut results: Vec<ui::PackageSummary> = Vec::new();
            let mut cleaned_packages: Vec<String> = Vec::new();
            let targets_vec: Vec<InstalledPackage> = removals.into_iter().collect();
            let mut cleanup_results: Vec<Option<Result<(), LkpmError>>> = Vec::with_capacity(targets_vec.len());
            for _ in 0..targets_vec.len() {
                cleanup_results.push(None);
            }
            let concurrency_limit = cfg.parallel_operation.max(1);
            let mut handles: Vec<std::thread::JoinHandle<(usize, Result<(), LkpmError>)>> = Vec::new();
            for (i, rec) in targets_vec.iter().enumerate() {
                let cfg_clone = cfg.clone();
                let rec_clone = rec.clone();
                handles.push(std::thread::spawn(move || {
                    let res = cleanup_package_assets(&cfg_clone, &rec_clone, None);
                    (i, res)
                }));
                if handles.len() >= concurrency_limit {
                    let h = handles.remove(0);
                    let (idx, res) = h.join().map_err(|_| LkpmError::Other("Cleanup thread panicked!".into()))?;
                    cleanup_results[idx] = Some(res);
                }
            }
            while let Some(h) = handles.pop() {
                let (idx, res) = h.join().map_err(|_| LkpmError::Other("Cleanup thread panicked!".into()))?;
                cleanup_results[idx] = Some(res);
            }
            for i in 0..targets_vec.len() {
                let record = targets_vec[i].clone();
                let size: u64 = record
                    .files
                    .iter()
                    .filter_map(|file| fs::metadata(file).ok().map(|m| m.len()))
                    .sum();
                ui::info(&format!("{}{}{}", "➔ Deleting ".bright_cyan(), record.name.bright_yellow(), " from the system....".bright_cyan()));
                let mut status = "deleted".to_string();
                let res_opt = cleanup_results[i].take();
                if let Some(res) = res_opt {
                    if let Err(e) = res {
                        status = format!("error: {}", e);
                    } else {
                        cleaned_packages.push(record.name.clone());
                    }
                } else {
                    status = "error: cleanup result missing".to_string();
                }
                results.push(ui::PackageSummary {
                    name: record.name.clone(),
                    version: record.version.clone(),
                    source: repo::repo_source_label(&cfg, &record.source),
                    size,
                    duration: duration.elapsed(),
                    checksum: record.checksum.clone(),
                    status,
                });
            }
            for package in cleaned_packages.iter() {
                if db.remove(&cfg, package)?.is_none()
                    && let Some(result) = results.iter_mut().find(|entry| entry.name == *package)
                {
                    result.status = format!("Error while deleting {}: package record is missing!", *package);
                }
            }
            ui::print_operation_summary(&results);
            Ok(())
        }
        Command::Refresh { .. } => {
            require_root_for_install_root(&cfg)?;
            ui::info("Refreshing repository metadata....");
            fs::create_dir_all(&cfg.cache_path).map_err(LkpmError::Io)?;
            let refreshed = repo::refresh_repo_metadata(&cfg)?;
            if refreshed.is_empty() {
                if cfg.core_repos.is_empty() && cfg.extra_mirrors.is_empty() {
                    ui::error("No main repository that has been configured!");
                } else {
                    ui::warning("Repository metadata refresh completed but no metadata files were refreshed!");
                }
            } else {
                ui::success("Repository metadata has been refreshed successfully!");
            }
            check_repository_connections(&cfg)?;
            if refreshed.is_empty() {
                ui::warning("Repository host is reachable but metadata cache could not be refreshed!");
            } else {
                ui::success("Repository metadata cache is ready.");
            }
            Ok(())
        }
        Command::Update {
            packages,
            noconfirm,
            ..
        } => {
            require_root_for_install_root(&cfg)?;
            ui::info("Checking for system update....");
            let duration = Instant::now();
            let mut installed_packages: Vec<InstalledPackage> = Vec::new();
            if packages.is_empty() {
                installed_packages = db.list()?;
            } else {
                for pkg_name in packages.iter() {
                    if let Some(rec) = find_installed_package(&db, pkg_name)? {
                        installed_packages.push(rec);
                    }
                }
            }
            if installed_packages.is_empty() {
                ui::warning("No installed packages found to update!");
                return Ok(());
            }
            for record in installed_packages.iter() {
                ensure_package_is_not_blocked(&cfg, &record.name)?;
            }
            struct UpdateTarget {
                record: InstalledPackage,
                location: repo::PackageLocation,
                remote_version: String,
            }
            let mut update_targets = Vec::new();
            for record in installed_packages.into_iter() {
                if let Ok(Some(repo_pkg)) = repo::find_repo_package_info(&cfg, &record.name) {
                    if repo_pkg.version != record.version {
                        if package_list_contains(&cfg.no_update_packages, &record.name) {
                            continue;
                        }
                        if let Ok(Some(location)) = repo::find_pkg_in_repos_location(&cfg, &record.name) {
                            update_targets.push(UpdateTarget {
                                record,
                                location,
                                remote_version: repo_pkg.version,
                            });
                        }
                    }
                }
            }
            if update_targets.is_empty() {
                ui::success("No updates available at this time.");
                return Ok(());
            }
            ui::info(&format!("Available updates ({}):", update_targets.len()));
            for target in update_targets.iter() {
                println!(
                    "    ‣ {} ({} ➔ {})",
                    target.record.name.bright_yellow(),
                    target.record.version.bright_red(),
                    target.remote_version.bright_green()
                );
            }
            let default = true;
            if !confirm_operation(
                noconfirm,
                "These package(s) will be updated to the new version. Proceed to update?",
                default,
            ) {
                ui::error("Update aborted by the user.");
                return Ok(());
            }
            let mut results: Vec<ui::PackageSummary> = Vec::new();
            ui::info(&format!(
                "{} {} {}",
                "Updating".bright_cyan(),
                update_targets.len(),
                "package(s)....".bright_cyan()
            ));
            let urls: Vec<String> = update_targets
                .iter()
                .map(|t| match &t.location {
                    repo::PackageLocation::Remote(u) => u.clone(),
                })
                .collect();
            let downloaded_paths = download_packages_concurrently(&cfg, &urls)?;
            let mut prepared_updates = Vec::new();
            for (i, target) in update_targets.into_iter().enumerate() {
                if let Some(path) = &downloaded_paths[i] {
                    let path = path.clone();
                    let checksum = match file_sha256(&path) {
                        Ok(sha) => sha,
                        Err(err) => {
                            ui::warning(&format!("Failed to calculate checksum for {}: {}", target.record.name, err));
                            continue;
                        }
                    };
                    let metadata = pkg::read_package_metadata(&path)?;
                    let size = package_size(&path);
                    validate_package_metadata(&cfg, &metadata)?;
                    prepared_updates.push((target, path, metadata, size, checksum));
                } else {
                    continue;
                }
            }
            ui::info(&format!(
                "{} {} {}",
                "Updating".bright_cyan(),
                prepared_updates.len(),
                "package(s) to system:".bright_cyan()
            ));
            // Phase 1: extract every update first, deferring post_upgrade
            // hooks to phase 2 so they only run once the whole batch of new
            // files is on disk.
            struct PendingUpdate {
                target: UpdateTarget,
                path: PathBuf,
                size: u64,
                checksum: String,
                extracted: ExtractedPackage,
            }
            let mut pending: Vec<PendingUpdate> = Vec::new();
            for (target, path, metadata, size, checksum) in prepared_updates.into_iter() {
                ui::info(&format!(
                    "➔ Updating {} ({} ➔ {})...",
                    target.record.name.bright_yellow(),
                    target.record.version.bright_yellow(),
                    metadata.version.bright_green()
                ));
                match extract_package(&cfg, &db, &metadata, &path) {
                    Ok(extracted) => {
                        let mut updated = target.record.clone();
                        updated.package_path = path.clone();
                        updated.version = extracted.metadata.version.clone();
                        updated.checksum = checksum.clone();
                        updated.files = extracted.installed_files.clone();
                        updated.requires = extracted.metadata.depends.clone();
                        updated.optdepends = extracted.metadata.optdepends.clone();
                        updated.conflicts = extracted.metadata.conflicts.clone();
                        updated.backups = extracted.backups_paths.clone();
                        updated.backups_hashes = extracted.backups_hashes.clone();
                        updated.install_script = extracted.install_script.clone();
                        db.register(&cfg, updated)?;
                        pending.push(PendingUpdate { target, path, size, checksum, extracted });
                    }
                    Err(err) => {
                        for item in pending.into_iter().rev() {
                            rollback_extracted_package(&cfg, &mut db, &item.extracted);
                        }
                        crate::downloader::clear_download_dir(&cfg);
                        return Err(LkpmError::Other(format!(
                            "Failed to extract {} ({}): {}. Aborting the whole update! No post_upgrade hooks were run for any package in this batch.",
                            target.record.name, metadata.version, err
                        )));
                    }
                }
            }
            // Phase 2: run post_upgrade now that every updated package has
            // its new files in place.
            ui::info(&format!(
                "{} {} {}",
                "Running post-upgrade hooks for".bright_cyan(),
                pending.len(),
                "package(s)....".bright_cyan()
            ));
            for item in pending.into_iter() {
                let hook_failure = run_post_install_hook(&cfg, &item.extracted);
                remove_remote_source(&item.target.record.source, &item.path);
                let status = match hook_failure {
                    Some(reason) => format!("updated (warning: {})", reason),
                    None => "updated".to_string(),
                };
                results.push(ui::PackageSummary {
                    name: item.target.record.name.clone(),
                    version: item.extracted.metadata.version.clone(),
                    source: repo::repo_source_label(&cfg, &item.target.record.source),
                    size: item.size,
                    duration: duration.elapsed(),
                    checksum: item.checksum,
                    status,
                });
            }
            ui::print_operation_summary(&results);
            Ok(())
        }
        Command::UpdateRefresh {
            packages,
            noconfirm,
            root,
        } => {
            require_root_for_install_root(&cfg)?;
            ui::info("Refreshing repository metadata...");
            let refreshed = repo::refresh_repo_metadata(&cfg)?;
            if refreshed.is_empty() {
                if cfg.core_repos.is_empty() && cfg.extra_mirrors.is_empty() {
                    ui::error("No main repository that has been configured!");
                } else {
                    ui::warning("Repository metadata refresh completed but no metadata files were refreshed!");
                }
            } else {
                ui::success("Repository metadata has been refreshed successfully!");
            }
            check_repository_connections(&cfg)?;
            if refreshed.is_empty() {
                ui::warning("Repository host is reachable but metadata cache could not be refreshed!");
            } else {
                ui::success("Repository metadata cache is ready.");
            }
            let default = true;
            if !confirm_operation(noconfirm, "Proceed with update after refresh?", default) {
                ui::error("Update after refresh aborted by the user.");
                return Ok(());
            }
            handle(Command::Update {
                packages: packages.clone(),
                noconfirm,
                root: root.clone(),
            })
        }
        Command::Package { package, .. } => {
            require_root_for_install_root(&cfg)?;
            ui::start_operation(&format!("Fetching {} information....", package));
            ensure_package_is_not_blocked(&cfg, &package)?;
            let record = if let Some(installed) = find_installed_package(&db, &package)? {
                Some(installed)
            } else if let Some(repo_pkg) = repo::find_repo_package_info(&cfg, &package)? {
                ui::warning(&format!(
                    "{} is not installed on the system. But metadata was found in the repository.",
                    package
                ));
                Some(InstalledPackage {
                    name: repo_pkg.name.clone(),
                    version: repo_pkg.version.clone(),
                    source: repo_pkg.origin.clone().unwrap_or_default(),
                    source_kind: "remote".into(),
                    package_path: PathBuf::new(),
                    checksum: String::new(),
                    files: Vec::new(),
                    requires: repo_pkg.depends.clone(),
                    optdepends: repo_pkg.optdepends.clone(),
                    conflicts: repo_pkg.conflicts.clone(),
                    provides: repo_pkg.provides.clone(),
                    backups: Vec::new(),
                    backups_hashes: HashMap::new(),
                    install_script: None,
                })
            } else {
                None
            };
            let record = record.ok_or_else(|| {
                LkpmError::Other(format!(
                    "{} is not installed on the system and no metadata was found in any configured repository!",
                    package
                ))
            })?;
            if record.requires.is_empty() && record.optdepends.is_empty() && record.conflicts.is_empty() {
                ui::success(&format!("All good! No dependencies or conflicts found for {}.", package));
            } else {
                let mut required: Vec<(&str, bool)> = Vec::new();
                for dep in record.requires.iter() {
                    required.push((dep.as_str(), find_installed_package(&db, dep)?.is_some()));
                }
                let optional = record.optdepends.iter().map(|s| s.as_str()).collect::<Vec<_>>();
                ui::dependency_report(&required, &optional);
                if !record.conflicts.is_empty() {
                    let conflicts = record.conflicts.iter().map(|s| s.as_str()).collect::<Vec<_>>();
                    ui::conflict_report(&conflicts);
                }
            }
            Ok(())
        }
        Command::Help => {
            println!("");
            println!("-----------------------------------------");
            println!("::: [ Liska Package Manager (1.1.0) ] :::");
            println!("-----------------------------------------");
            println!("");
            println!("Usage: lkpm <command> [options]");
            println!("> -i <pkgs>                  install a package(s) to the system");
            println!("> -id | -di <pkgs>           install a package(s) and its dependencies to the system");
            println!("> -l <files>                 install a local package(s) directly from local directory, bypassing repositories");
            println!("> -ld | -dl <files>          install a local package(s) and resolve missing dependencies from repositories");
            println!("> -u                         update the system by checking for updates to installed package(s)");
            println!("> -d <pkgs>                  delete installed package(s) from the system");
            println!("> -r                         refresh repository metadata");
            println!("> -ru | -ur <pkgs>           refresh repository metadata then update the system or specified package(s)");
            println!("> -p <pkg>                   show package dependencies and conflicts information");
            println!("> --noconfirm                skip interactive prompts and proceed with operation");
            println!("> --root=<path>              run lkpm command under another root directory");
            println!("> /etc/lkpm.d/config.lua     lkpm behaviour file configuration");
            println!("> /etc/lkpm.d/mirrorlist     lkpm mirrorlist file configuration");
            println!("");
            Ok(())
        }
    }
}

fn is_hidden_path(path: &Path) -> bool {
    path.components().any(|cmp| match cmp {
        Component::Normal(name) => name.to_string_lossy().starts_with('.'),
        _ => false,
    })
}

fn cleanup_package_assets(
    cfg: &Config,
    record: &InstalledPackage,
    pb: Option<&ProgressBar>,
) -> Result<(), LkpmError> {
    if let Err(e) = pkg::run_install_hook(
        cfg,
        &record.name,
        record.install_script.as_deref(),
        "pre_remove",
        &[record.version.as_str()],
    ) {
        ui::warning(&format!("pre_remove hook failed for {}: {}", record.name, e));
    }
    if record.package_path.exists() {
        fs::remove_file(&record.package_path).map_err(LkpmError::Io)?;
    }
    let mut removed = 0u64;
    for file in record.files.iter() {
        let file_size = fs::metadata(file).ok().map(|m| m.len()).unwrap_or(0);
        if is_hidden_path(file) {
            if let Some(pb) = pb {
                removed += file_size;
                pb.set_position(removed);
            }
            continue;
        }
        if record.backups.contains(file) && file.exists() {
            let relative_key = file
                .strip_prefix(&cfg.install_root)
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_else(|_| file.to_string_lossy().to_string());
            let pristine_hash = record.backups_hashes.get(&relative_key);
            let current_hash = fs::read(file).ok().map(|data| {
                let mut hasher = Sha256::new();
                hasher.update(&data);
                hex::encode(hasher.finalize())
            });
            let modified = matches!((&current_hash, pristine_hash), (Some(current), Some(pristine)) if current != pristine);
            if modified {
                let saved_name = format!(
                    "{}.lkpmsave",
                    file.file_name().unwrap_or_default().to_string_lossy()
                );
                let saved_path = file.with_file_name(saved_name);
                if let Err(e) = fs::rename(file, &saved_path) {
                    return Err(LkpmError::Io(e));
                }
                ui::warning(&format!(
                    "{} was modified locally! Kept saved as {}.",
                    file.display(),
                    saved_path.display()
                ));
            } else if let Err(e) = fs::remove_file(file) {
                return Err(LkpmError::Io(e));
            }
            if let Some(pb) = pb {
                removed += file_size;
                pb.set_position(removed);
            }
            continue;
        }
        if file.exists() && let Err(e) = fs::remove_file(file) {
            return Err(LkpmError::Io(e));
        }
        if let Some(pb) = pb {
            removed += file_size;
            pb.set_position(removed);
        }
    }
    if let Some(pb) = pb {
        pb.finish();
    }
    if let Err(e) = pkg::run_install_hook(
        cfg,
        &record.name,
        record.install_script.as_deref(),
        "post_remove",
        &[record.version.as_str()],
    ) {
        ui::warning(&format!("post_remove hook failed for {}: {}", record.name, e));
    }
    Ok(())
}
