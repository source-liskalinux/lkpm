use crate::config::Config;
use crate::downloader::download_to;
use crate::database;
use crate::error::LkpmError;
use reqwest::blocking::Client;
use serde::Deserialize;
use serde_json;
use std::fs;
use std::io::{Cursor, Read};
use std::path::PathBuf;

const SUPPORTED_PACKAGE_EXTENSIONS: &[&str] = &[
    "pkg.tar.zst",
    "pkg.tar.xz",
    "pkg.tar.gz",
    "lsk.tar.zst",
    "lsk.tar.xz",
    "lsk.tar.gz",
    "tar.zst",
    "tar.xz",
    "tar.gz",
    "tar",
];

#[derive(Clone)]
pub enum PackageLocation {
    Remote(String),
}

#[derive(Debug, PartialEq, Eq)]
struct PackageFileMetadata {
    name: String,
    version: String,
}

#[derive(Debug, Clone, Deserialize)]
struct JsonRepoIndex {
    packages: Vec<JsonRepoPackage>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct JsonRepoPackage {
    pub name: String,
    pub version: String,
    pub origin: Option<String>,
    pub sha256: Option<String>,
    pub url: String,
    #[serde(default)]
    pub provides: Vec<String>,
    #[serde(default)]
    pub depends: Vec<String>,
    #[serde(default)]
    pub optdepends: Vec<String>,
    #[serde(default)]
    pub conflicts: Vec<String>,
}

fn normalize_url(base: &str, href: &str) -> String {
    if href.starts_with("http://") || href.starts_with("https://") {
        href.to_string()
    } else if href.starts_with('/') {
        let prefix = base.split('/').take(3).collect::<Vec<_>>().join("/");
        format!("{}{}", prefix, href)
    } else {
        format!(
            "{}/{}",
            base.trim_end_matches('/'),
            href.trim_start_matches('/')
        )
    }
}

fn file_name_from_href(href: &str) -> String {
    href.split('?')
    .next()
        .unwrap_or(href)
        .trim_end_matches('/')
        .split('/')
    .next_back()
        .unwrap_or(href)
        .to_string()
}

pub fn canonical_package_name(package: &str) -> &str {
    match package {
        "kernel" => "linux",
        "sh" => "bash",
        "systemd" => "lksystem",
        "bsdtar" => "libarchive",
        _ => package,
    }
}

fn soname_root(name: &str) -> Option<&str> {
    let lower = name.to_ascii_lowercase();
    if !lower.starts_with("lib") {
        return None;
    }
    lower.find(".so").map(|idx| &name[..idx])
}

pub fn package_name_matches_request(requested: &str, actual: &str) -> bool {
    let req_clean = crate::pkg_utils::normalize_dependency_name(requested);
    let act_clean = crate::pkg_utils::normalize_dependency_name(actual);
    let requested_lower = req_clean.to_ascii_lowercase();
    let actual_lower = act_clean.to_ascii_lowercase();
    if actual_lower == requested_lower {
        return true;
    }
    if actual_lower == canonical_package_name(&req_clean).to_ascii_lowercase() {
        return true;
    }
    if let Some(req_root) = soname_root(&req_clean) {
        if actual_lower == req_root
            || actual_lower == format!("{}.so", req_root)
            || actual_lower.starts_with(&format!("{}.so.", req_root))
        {
            return true;
        }
    }
    if let Some(act_root) = soname_root(&act_clean) {
        if requested_lower == act_root
            || requested_lower == format!("{}.so", act_root)
            || requested_lower.starts_with(&format!("{}.so.", act_root))
        {
            return true;
        }
    }
    false
}

fn strip_package_extension(file_name: &str) -> Option<&str> {
    let lower = file_name.to_lowercase();
    for ext in SUPPORTED_PACKAGE_EXTENSIONS {
        let suffix = format!(".{}", ext);
        if lower.ends_with(&suffix) {
            return file_name.get(..file_name.len() - suffix.len());
        }
    }
    None
}

fn looks_like_version(value: &str) -> bool {
    value
        .chars()
        .next()
        .map(|c| c.is_ascii_digit())
        .unwrap_or(false)
}

fn looks_like_arch(value: &str) -> bool {
    matches!(
        value,
        "any" | "x86_64" | "i686" | "aarch64" | "armv7h" | "riscv64"
    )
}

fn parse_package_file_metadata(file_name: &str) -> Option<PackageFileMetadata> {
    let stem = strip_package_extension(file_name)?;
    let arch_parts: Vec<&str> = stem.rsplitn(4, '-').collect();
    if arch_parts.len() == 4 && looks_like_arch(arch_parts[0]) && looks_like_version(arch_parts[2])
    {
        return Some(PackageFileMetadata {
            name: arch_parts[3].to_string(),
            version: format!("{}-{}", arch_parts[2], arch_parts[1]),
        });
    }
    if arch_parts.len() == 3 && looks_like_arch(arch_parts[0]) && looks_like_version(arch_parts[1])
    {
        return Some(PackageFileMetadata {
            name: arch_parts[2].to_string(),
            version: arch_parts[1].to_string(),
        });
    }
    if let Some((name, version)) = stem.rsplit_once('-') && looks_like_version(version) {
        return Some(PackageFileMetadata {
            name: name.to_string(),
            version: version.to_string(),
        });
    }
    None
}

pub fn package_name_from_file_name(file_name: &str) -> String {
    parse_package_file_metadata(file_name)
        .map(|metadata| metadata.name)
        .unwrap_or_else(|| file_name.to_string())
}

pub fn extract_package_version(package: &str, file_name: &str) -> Option<String> {
    parse_package_file_metadata(file_name).and_then(|metadata| {
        if package_name_matches_request(package, &metadata.name) {
            Some(metadata.version)
        } else {
            None
        }
    })
}

pub fn package_file_name_from_url(url: &str) -> String {
    let cleaned_url = url
        .split('?')
        .next()
        .unwrap_or(url)
        .trim_end_matches('/');
    file_name_from_href(cleaned_url)
}

fn repo_match_prefix(repo_url: &str) -> &str {
    let trimmed = repo_url.trim_end_matches('/');
    if trimmed.ends_with(".json") {
        trimmed.rsplit_once('/').map(|(base, _)| base).unwrap_or(trimmed)
    } else {
        trimmed
    }
}

pub fn repo_source_label(cfg: &Config, source: &str) -> String {
    for repo_url in cfg.core_repos.iter().chain(cfg.extra_mirrors.iter()) {
        let prefix = repo_match_prefix(repo_url);
        if source.starts_with(prefix) {
            if let Ok(parsed) = reqwest::Url::parse(prefix) {
                if let Some(host) = parsed.host_str() {
                    return host.to_string();
                }
            }
            return prefix.to_string();
        }
    }
    reqwest::Url::parse(source)
        .ok()
        .and_then(|url| url.host_str().map(|host| host.to_string()))
        .unwrap_or_else(|| source.to_string())
}

pub fn repo_roots(cfg: &Config) -> Vec<String> {
    cfg.core_repos
        .iter()
        .chain(cfg.extra_mirrors.iter())
        .map(|mirror| format!("{}/", mirror.trim_end_matches('/')))
        .collect()
}

pub fn repo_is_reachable(url: &str) -> Result<bool, LkpmError> {
    let client = Client::builder().build().map_err(|e| LkpmError::Network(e.to_string()))?;
    let index_url = json_repo_index_url(url);
    let urls = if index_url == url {
        vec![url.to_string()]
    } else {
        vec![index_url, url.to_string()]
    };
    for target in urls.iter() {
        if let Ok(resp) = client.get(target).send() {
            if resp.status().is_success() {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

pub fn find_pkg_in_repos_location(
    cfg: &Config,
    package: &str,
) -> Result<Option<PackageLocation>, LkpmError> {
    find_pkg_in_json_repos(cfg, package)
}

pub fn refresh_repo_metadata(cfg: &Config) -> Result<Vec<PathBuf>, LkpmError> {
    fs::create_dir_all(&cfg.cache_path).map_err(LkpmError::Io)?;
    let mut refreshed = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    let mut repo_found = false;
    for repo_url in cfg.core_repos.iter().chain(cfg.extra_mirrors.iter()) {
        repo_found = true;
        match refresh_json_repo(cfg, repo_url) {
            Ok(Some(path)) => refreshed.push(path),
            Ok(None) => errors.push(format!("{}: metadata refresh returned no data", repo_url)),
            Err(err) => errors.push(format!("{}: {}", repo_url, err)),
        }
    }
    if !repo_found {
        return Ok(refreshed);
    }
    if refreshed.is_empty() {
        let message = if errors.is_empty() {
            "Unable to refresh repository metadata.".to_string()
        } else {
            format!("Failed to refresh any configured repositories: {}", errors.join("; "))
        };
        Err(LkpmError::Network(message))
    } else {
        Ok(refreshed)
    }
}

fn json_repo_index_url(repo_url: &str) -> String {
    let repo = repo_url.trim_end_matches('/');
    if repo.ends_with(".json.tar.zst") {
        repo.to_string()
    } else {
        format!("{}/db.json.tar.zst", repo)
    }
}

fn json_repo_base_url(repo_url: &str) -> String {
    let repo = repo_url.trim_end_matches('/');
    if repo.ends_with(".json.tar.zst") {
        repo.rsplit_once('/').map(|x| x.0).unwrap_or(repo).to_string()
    } else {
        repo.to_string()
    }
}

fn refresh_json_repo(cfg: &Config, repo_url: &str) -> Result<Option<PathBuf>, LkpmError> {
    let index_url = json_repo_index_url(repo_url);
    let client = Client::builder().build().map_err(|e| LkpmError::Network(e.to_string()))?;
    let resp = client.get(&index_url).send();
    match resp {
        Ok(r) if r.status().is_success() => {
            let text = if index_url.ends_with(".json.tar.zst") {
                let bytes = r.bytes().map_err(|e| LkpmError::Network(e.to_string()))?;
                let mut decoder = zstd::Decoder::new(Cursor::new(bytes))
                    .map_err(|e| LkpmError::Network(e.to_string()))?;
                let mut archive = tar::Archive::new(&mut decoder);
                let mut db_json = String::new();
                let mut found = false;
                for entry in archive.entries().map_err(|e| LkpmError::Network(e.to_string()))? {
                    let mut entry = entry.map_err(|e| LkpmError::Network(e.to_string()))?;
                    let path = entry.path().map_err(|e| LkpmError::Network(e.to_string()))?;
                    if path.file_name().and_then(|name| name.to_str()) == Some("db.json") {
                        entry.read_to_string(&mut db_json).map_err(|e| LkpmError::Network(e.to_string()))?;
                        found = true;
                        break;
                    }
                }
                if !found {
                    return Err(LkpmError::Network(format!("db.json not found in {}", index_url)));
                }
                db_json
            } else {
                r.text().map_err(|e| LkpmError::Network(e.to_string()))?
            };
            database::Database::store_repo_index(cfg, &index_url, &text)?;
            Ok(Some(cfg.db_path.join("db.sqlite")))
        }
        Ok(_) => Err(LkpmError::Network(format!("Failed to fetch {}", index_url))),
        Err(err) => Err(LkpmError::Network(err.to_string())),
    }
}

fn read_or_refresh_json_repo(cfg: &Config, repo_url: &str) -> Result<String, LkpmError> {
    let index_url = json_repo_index_url(repo_url);
    if let Some(json) = database::Database::read_repo_index(cfg, &index_url)? {
        return Ok(json);
    }
    // Try to refresh and then read from DB
    if let Some(_) = refresh_json_repo(cfg, repo_url)? {
        if let Some(json) = database::Database::read_repo_index(cfg, &index_url)? {
            return Ok(json);
        }
    }
    Err(LkpmError::Other(format!("Repository index not available for {}", repo_url)))
}

fn normalize_json_package_url(repo_url: &str, package_url: &str) -> String {
    if package_url.starts_with("http://") || package_url.starts_with("https://") {
        package_url.to_string()
    } else {
        normalize_url(&json_repo_base_url(repo_url), package_url)
    }
}

fn parse_repo_packages(json: &str) -> Result<Vec<JsonRepoPackage>, serde_json::Error> {
    if let Ok(index) = serde_json::from_str::<JsonRepoIndex>(json) {
        return Ok(index.packages);
    }
    serde_json::from_str::<Vec<JsonRepoPackage>>(json)
}

pub fn find_repo_package_info(
    cfg: &Config,
    package: &str,
) -> Result<Option<JsonRepoPackage>, LkpmError> {
    for repo_url in cfg.core_repos.iter().chain(cfg.extra_mirrors.iter()) {
        let json = match read_or_refresh_json_repo(cfg, repo_url) {
            Ok(json) => json,
            Err(_) => continue,
        };
        let packages = match parse_repo_packages(&json) {
            Ok(packages) => packages,
            Err(_) => continue,
        };
        for pkg in packages.iter() {
            let matches_name = package_name_matches_request(package, &pkg.name);
            let matches_provide = pkg
                .provides
                .iter()
                .any(|provide| package_name_matches_request(package, provide));
            if matches_name || matches_provide {
                return Ok(Some(pkg.clone()));
            }
        }
    }
    Ok(None)
}

fn find_pkg_in_json_repos(
    cfg: &Config,
    package: &str,
) -> Result<Option<PackageLocation>, LkpmError> {
    for repo_url in cfg.core_repos.iter().chain(cfg.extra_mirrors.iter()) {
        let json = match read_or_refresh_json_repo(cfg, repo_url) {
            Ok(json) => json,
            Err(_) => continue,
        };
        let packages = match parse_repo_packages(&json) {
            Ok(packages) => packages,
            Err(_) => continue,
        };
        for pkg in packages.iter() {
            let matches_name = package_name_matches_request(package, &pkg.name);
            let matches_provide = pkg
                .provides
                .iter()
                .any(|provide| package_name_matches_request(package, provide));
            if matches_name || matches_provide {
                let package_url = normalize_json_package_url(repo_url, &pkg.url);
                return Ok(Some(PackageLocation::Remote(package_url)));
            }
        }
    }
    Ok(None)
}

pub fn download_pkg_url(
    cfg: &Config, 
    url: &str, 
    pb: Option<indicatif::ProgressBar>,
    overall_pb: Option<indicatif::ProgressBar>,
) -> Result<PathBuf, LkpmError> {
    let file_name = package_file_name_from_url(url);
    let local = cfg.cache_path.join(&file_name);
    fs::create_dir_all(&cfg.cache_path).map_err(LkpmError::Io)?;
    downloader::download_to(url, &local, pb.as_ref(), overall_pb.as_ref())
        .map_err(|e| LkpmError::Network(e.to_string()))
}
