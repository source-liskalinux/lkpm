use std::env;
use std::fs;
use std::io::Write;
use libc;
use std::path::Path;
use std::collections::{HashMap, HashSet, VecDeque};
use std::process::{Command, exit};
use colored::*;

fn log(msg: &str) { println!("{} {}", "::: [ LKMAKE ] ::: (i) >".bright_cyan(), msg); }
fn log_success(msg: &str) { println!("{} {}", "::: [ LKMAKE ] ::: (✓) >".bright_green(), msg.bright_green()); }
fn log_warn(msg: &str) { println!("{} {}", "::: [ LKMAKE ] ::: (!) >".bright_yellow(), msg.bright_yellow()); }
fn log_error(msg: &str) { println!("{} {}", "::: [ LKMAKE ] ::: (✗) >".bright_red(), msg.bright_red()); }

fn run_bash_capture(cmd: &str) -> Option<String> {
    let out = Command::new("bash").arg("-c").arg(cmd).output().ok()?;
    if out.status.success() { Some(String::from_utf8_lossy(&out.stdout).to_string()) } else { None }
}

fn resolve_and_install_deps(initial_deps: Vec<String>) {
    let mut queue: VecDeque<String> = VecDeque::from(initial_deps);
    let mut processed: HashSet<String> = HashSet::new();
    log("Resolving build dependencies....");
    while let Some(pkg) = queue.pop_front() {
        if processed.contains(&pkg) { continue; }
        log(&format!("Installing {}....", pkg));
        let status = run_lkpm(&["-id", &pkg, "--noconfirm"]);
        match status {
            Ok(s) if s.success() => { processed.insert(pkg.clone()); }
            _ => { log_error(&format!("Failed to install {}. Skipping....", pkg)); processed.insert(pkg.clone()); }
        }
    }
}

fn array_from_pkgbuild(var: &str) -> Vec<String> {
    if !Path::new("PKGBUILD").exists() { log("PKGBUILD not found"); exit(1); }
    let pkgbuild = canonical_path("PKGBUILD");
    let script = format!("source \"{}\" >/dev/null 2>&1 && printf '%s\\n' \"${{{}[@]}}\"", pkgbuild, var);
    let out = run_bash_capture(&script).unwrap_or_default();
    out.lines().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
}

fn download_sources(srcdir: &str) -> Vec<String> {
    fs::create_dir_all(srcdir).ok();
    let sources = array_from_pkgbuild("source");
    let mut paths = Vec::new();
    for s in sources {
        let filename = s.rsplit('/').next().unwrap_or(&s);
        let dest = format!("{}/{}", srcdir, filename);
        if Path::new(&dest).exists() { paths.push(dest.clone()); continue; }
        log(&format!("Downloading {}....", s));
        let status = Command::new("curl").args(&["-L","-f","-s","-S","-o", &dest, &s]).status();
        match status { Ok(st) if st.success() => { paths.push(dest.clone()); } _ => { log_error(&format!("Failed to download {}!", s)); } }
    }
    paths
}

fn sha256_of(path: &str) -> Option<String> {
    let out = Command::new("sha256sum").arg(path).output().ok()?;
    if !out.status.success() { return None }
    let s = String::from_utf8_lossy(&out.stdout);
    Some(s.split_whitespace().next().unwrap_or("").to_string())
}

fn check_integrity(srcdir: &str) -> bool {
    let sums = array_from_pkgbuild("sha256sums");
    if sums.is_empty() { log_warn("No checksums found. Skipping integrity check...."); return true }
    let mut expected: HashMap<String, String> = HashMap::new();
    for entry in sums {
        let entry = entry.trim();
        if entry.is_empty() || entry == "SKIP" { continue; }
        let mut parts = entry.split_whitespace();
        let sha = parts.next();
        let filename = parts.next();
        if let (Some(sha), Some(filename)) = (sha, filename) {
            expected.insert(filename.to_string(), sha.to_string());
        } else {
            log_warn(&format!("Invalid sha256sums entry: {}", entry));
        }
    }
    if expected.is_empty() { log_warn("No valid sha256sums found. Skipping integrity check...."); return true }
    let mut all_good = true;
    for (name, expected_hash) in expected.iter() {
        let path = format!("{}/{}", srcdir, name);
        if !Path::new(&path).exists() {
            log_error(&format!("Expected source file {} is missing", name));
            all_good = false;
            continue;
        }
        match sha256_of(&path) {
            Some(actual_hash) => {
                if actual_hash != *expected_hash {
                    log_error(&format!("Checksum mismatch for {}: expected {} got {}", name, expected_hash, actual_hash));
                    all_good = false;
                }
            }
            None => {
                log_error(&format!("Failed to compute sha256 for {}", name));
                all_good = false;
            }
        }
    }
    all_good
}

fn extract_sources(srcdir: &str) -> bool {
    let sources = array_from_pkgbuild("source");
    if sources.is_empty() {
        log_warn("No source array in PKGBUILD. Skipping source extraction.");
        return true;
    }
    for source in sources {
        let filename = source.rsplit('/').next().unwrap_or(&source);
        let path = format!("{}/{}", srcdir, filename);
        if !Path::new(&path).exists() {
            log_warn(&format!("Source file {} not found in {}. Skipping.", filename, srcdir));
            continue;
        }
        if Path::new(&path).is_file() {
            log(&format!("Extracting {}....", path));
            let status = Command::new("bsdtar").args(&["-xf", &path, "-C", srcdir]).status()
                .or_else(|_| Command::new("tar").args(&["-xf", &path, "-C", srcdir]).status());
            if status.is_err() { log_error(&format!("Failed to extract {}!", path)); }
        }
    }
    true
}

fn canonical_path(path: &str) -> String {
    fs::canonicalize(path)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| {
            env::current_dir()
                .map(|d| d.join(path).to_string_lossy().to_string())
                .unwrap_or_else(|_| path.to_string())
        })
}

fn pkg_function_exists(func: &str, cwd: &str) -> bool {
    let pkgbuild = canonical_path("PKGBUILD");
    let cmd = format!(
        "source \"{}\" >/dev/null 2>&1 && type -t \"{}\" >/dev/null 2>&1",
        pkgbuild,
        func
    );
    Command::new("bash").current_dir(cwd).arg("-c").arg(cmd).status().map(|s| s.success()).unwrap_or(false)
}

fn list_pkg_functions(prefix: &str, cwd: &str) -> Vec<String> {
    let pkgbuild = canonical_path("PKGBUILD");
    let cmd = format!(
        "source \"{}\" >/dev/null 2>&1 && declare -F | awk '{{print $3}}' | grep -E '^{}_+' | sort -u",
        pkgbuild,
        prefix
    );
    let out = Command::new("bash").current_dir(cwd).arg("-c").arg(cmd).output().ok();
    if let Some(out) = out {
        if out.status.success() {
            return String::from_utf8_lossy(&out.stdout)
                .lines()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
        }
    }
    Vec::new()
}

fn run_pkg_function(func: &str, cwd: &str, srcdir: &str, pkgdir: &str, pkgdest: &str, fakeroot_state: &str) -> bool {
    let srcdir_abs = canonical_path(srcdir);
    let pkgdir_abs = canonical_path(pkgdir);
    let pkgdest_abs = canonical_path(pkgdest);
    // Export shell variables so PKGBUILD functions that reference
    // $pkgdir, $srcdir, $PKGDEST and $SRCDEST write into the right locations.
    let pkgbuild = canonical_path("PKGBUILD");
    let cmd = format!(
        "PKGDEST=\"{}\" SRCDEST=\"{}\" srcdir=\"{}\" pkgdir=\"{}\"; export PKGDEST SRCDEST srcdir pkgdir; source \"{}\" >/dev/null 2>&1 && set -e && {}",
        pkgdest_abs, srcdir_abs, srcdir_abs, pkgdir_abs, pkgbuild, func
    );
    let mut command = if func.starts_with("package") {
        let mut c = Command::new("fakeroot");
        c.args(&["-i", fakeroot_state, "-s", fakeroot_state]);
        c.arg("bash");
        c
    } else {
        Command::new("bash")
    };
    let status = command.current_dir(cwd).arg("-c").arg(cmd).status();
    match status { Ok(s) => s.success(), Err(_) => false }
}

fn run_pkg_phase(phase: &str, cwd: &str, srcdir: &str, pkgdir: &str, pkgdest: &str, fakeroot_state: &str) -> bool {
    let mut funcs = Vec::new();
    if pkg_function_exists(phase, cwd) {
        funcs.push(phase.to_string());
    }
    funcs.extend(list_pkg_functions(phase, cwd));
    if funcs.is_empty() {
        return true;
    }
    for func in funcs {
        log(&format!("Running {}....", func));
        if !run_pkg_function(&func, cwd, srcdir, pkgdir, pkgdest, fakeroot_state) {
            log_error(&format!("{} failed! Aborting the build process.", func));
            exit(1);
        }
    }
    true
}

fn package_archive(pkgdir: &str, pkgname: &str, destdir: &str, fakeroot_state: &str) -> bool {
    fs::create_dir_all(destdir).ok();
    let archive = format!("{}/{}.lsk.tar.zst", destdir, pkgname);
    // Ensure files are readable so tar won't fail opening files owned by root
    let _ = Command::new("chmod").args(&["-R", "a+rX", pkgdir]).status();
    let out = Command::new("fakeroot")
        .args(&["-i", fakeroot_state])
        .args(&["tar", "-C", pkgdir, "-I", "zstd", "-cf", &archive, "."])
        .output()
        .or_else(|_| Command::new("fakeroot")
            .args(&["-i", fakeroot_state])
            .args(&["tar", "-C", pkgdir, "--zstd", "-cf", &archive, "."])
            .output());
    match out {
        Ok(o) => {
            if !o.status.success() {
                log_error(&format!("tar failed: {}", String::from_utf8_lossy(&o.stderr).trim()));
            }
            o.status.success()
        }
        Err(_) => false
    }
}

fn get_var(var: &str) -> Option<String> {
    run_bash_capture(&format!("source ./PKGBUILD >/dev/null 2>&1 && printf '%s' \"${{{}}}\"", var))
}

fn dir_size(path: &str) -> u64 {
    let out = run_bash_capture(&format!("du -sb {} 2>/dev/null | cut -f1", path)).unwrap_or_default();
    out.trim().parse().unwrap_or(0)
}

fn write_pkginfo(pkgdir: &str) {
    let mut pkgbase = get_var("pkgbase").unwrap_or_default();
    let pkgver = get_var("pkgver").unwrap_or_default();
    let pkgrel = get_var("pkgrel").unwrap_or_default();
    let fullver = format!("{}-{}", pkgver, pkgrel);
    let pkgnames = array_from_pkgbuild("pkgname");
    let mut pkgdesc = get_var("pkgdesc").unwrap_or_default();
    pkgdesc = pkgdesc.split_whitespace().collect::<Vec<_>>().join(" ");
    let url = get_var("url").unwrap_or_default();
    let packager = std::env::var("PACKAGER").unwrap_or_else(|_| "Unknown Packager".to_string());
    let builddate = std::env::var("SOURCE_DATE_EPOCH").unwrap_or_else(|_| format!("{}", chrono::Utc::now().timestamp()));
    let arch = array_from_pkgbuild("arch").get(0).cloned().unwrap_or_else(|| run_bash_capture("uname -m").unwrap_or_default());
    let license = array_from_pkgbuild("license");
    let replaces = array_from_pkgbuild("replaces");
    let groups = array_from_pkgbuild("groups");
    let conflicts = array_from_pkgbuild("conflicts");
    let provides = array_from_pkgbuild("provides");
    let backups = array_from_pkgbuild("backup");
    let depends = array_from_pkgbuild("depends");
    let optdepend = array_from_pkgbuild("optdepends");
    let makedepend = array_from_pkgbuild("makedepends");
    let checkdepend = array_from_pkgbuild("checkdepends");
    let xdata = array_from_pkgbuild("xdata");
    let pkgtype = get_var("pkgtype").map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).unwrap_or_else(|| "lsk".to_string());
    let size = dir_size(pkgdir);
    fs::create_dir_all(pkgdir).ok();
    let path = format!("{}/.PKGINFO", pkgdir);
    let fakeroot_ver = run_bash_capture("fakeroot -v").unwrap_or_else(|| "lkmake".to_string());
    let makepkg_version = env!("CARGO_PKG_VERSION");
    if let Ok(mut f) = fs::File::create(&path) {
        let _ = writeln!(f, "# Generated by lkmake {}", makepkg_version);
        let _ = writeln!(f, "# using {}", fakeroot_ver.trim());
        for pn in pkgnames.iter() {
            let _ = writeln!(f, "pkgname = {}", pn);
        }
        if pkgbase.is_empty() { pkgbase = pkgnames.get(0).cloned().unwrap_or_default(); }
        let _ = writeln!(f, "pkgbase = {}", pkgbase);
        if !xdata.is_empty() {
            let mut parts = vec![format!("pkgtype={}", pkgtype)];
            parts.extend(xdata);
            for p in parts {
                let _ = writeln!(f, "xdata = {}", p);
            }
        } else {
            let _ = writeln!(f, "xdata = pkgtype={}", pkgtype);
        }
        let _ = writeln!(f, "pkgver = {}", fullver);
        let _ = writeln!(f, "pkgdesc = {}", pkgdesc);
        let _ = writeln!(f, "url = {}", url);
        let _ = writeln!(f, "builddate = {}", builddate);
        let _ = writeln!(f, "packager = {}", packager);
        let _ = writeln!(f, "size = {}", size);
        let _ = writeln!(f, "arch = {}", arch);
        for v in license { let _ = writeln!(f, "license = {}", v); }
        for v in replaces { let _ = writeln!(f, "replaces = {}", v); }
        for v in groups { let _ = writeln!(f, "group = {}", v); }
        for v in conflicts { let _ = writeln!(f, "conflict = {}", v); }
        for v in provides { let _ = writeln!(f, "provides = {}", v); }
        for v in backups { let _ = writeln!(f, "backup = {}", v); }
        for v in depends { let _ = writeln!(f, "depend = {}", v); }
        for v in optdepend { let _ = writeln!(f, "optdepend = {}", v); }
        for v in makedepend { let _ = writeln!(f, "makedepend = {}", v); }
        for v in checkdepend { let _ = writeln!(f, "checkdepend = {}", v); }
    }
}

fn write_buildinfo(pkgdir: &str) {
    let pkgnames = array_from_pkgbuild("pkgname");
    let mut pkgbase = get_var("pkgbase").unwrap_or_default();
    let pkgver = get_var("pkgver").unwrap_or_default();
    let pkgrel = get_var("pkgrel").unwrap_or_default();
    let fullver = format!("{}-{}", pkgver, pkgrel);
    let pkgarch = array_from_pkgbuild("arch").get(0).cloned().unwrap_or_else(|| run_bash_capture("uname -m").unwrap_or_default());
    let builddir = std::env::var("BUILDDIR").unwrap_or_else(|_| std::env::current_dir().map(|d| d.to_string_lossy().to_string()).unwrap_or_else(|_| "/".to_string()));
    let startdir = std::env::current_dir().map(|d| d.to_string_lossy().to_string()).unwrap_or_default();
    let packager = std::env::var("PACKAGER").unwrap_or_else(|_| "Unknown Packager".to_string());
    let builddate = std::env::var("SOURCE_DATE_EPOCH").unwrap_or_else(|_| format!("{}", chrono::Utc::now().timestamp()));
    let bufile = std::env::var("BUILDFILE").unwrap_or_else(|_| "PKGBUILD".to_string());
    let sha = sha256_of(&bufile).unwrap_or_default();
    let buildenv = array_from_pkgbuild("BUILDENV");
    let options = array_from_pkgbuild("OPTIONS");
    fs::create_dir_all(pkgdir).ok();
    let path = format!("{}/.BUILDINFO", pkgdir);
    if let Ok(mut f) = fs::File::create(&path) {
        let _ = writeln!(f, "format = 2");
        for pn in pkgnames.iter() { let _ = writeln!(f, "pkgname = {}", pn); }
        if pkgbase.is_empty() { pkgbase = pkgnames.get(0).cloned().unwrap_or_default(); }
        let _ = writeln!(f, "pkgbase = {}", pkgbase);
        let _ = writeln!(f, "pkgver = {}", fullver);
        let _ = writeln!(f, "pkgarch = {}", pkgarch);
        let _ = writeln!(f, "pkgbuild_sha256sum = {}", sha);
        let _ = writeln!(f, "packager = {}", packager);
        let _ = writeln!(f, "builddate = {}", builddate);
        let _ = writeln!(f, "builddir = {}", builddir);
        let _ = writeln!(f, "startdir = {}", startdir);
        let _ = writeln!(f, "buildtool = {}", "lkmake");
        let _ = writeln!(f, "buildtoolver = {}", "1.0.0");
        for e in buildenv { let _ = writeln!(f, "buildenv = {}", e); }
        for o in options { let _ = writeln!(f, "options = {}", o); }
            let installed_parsed = run_bash_capture(r#"LC_ALL=C lkpm -Qi 2>/dev/null | awk -F': ' '/^Name .*/ {printf "%s", $2} /^Version .*/ {printf "-%s", $2} /^Architecture .*/ {print "-"$2}'"#)
                .or_else(|| run_bash_capture(r#"LC_ALL=C pacman -Qi 2>/dev/null | awk -F': ' '/^Name .*/ {printf "%s", $2} /^Version .*/ {printf "-%s", $2} /^Architecture .*/ {print "-"$2}'"#));
        if let Some(parsed) = installed_parsed {
            for line in parsed.lines() {
                if !line.trim().is_empty() { let _ = writeln!(f, "installed = {}", line.trim()); }
            }
        }
    }
}

fn read_pkgname() -> Option<String> {
    let pkgnames = array_from_pkgbuild("pkgname");
    let name = pkgnames.get(0).cloned().or_else(|| get_var("pkgname")).or_else(|| get_var("pkgbase")).unwrap_or_else(|| "package-unknown".to_string());
    let pkgver = get_var("pkgver").unwrap_or_default();
    let pkgrel = get_var("pkgrel").unwrap_or_default();
    let arch = array_from_pkgbuild("arch").get(0).cloned().unwrap_or_else(|| run_bash_capture("uname -m").unwrap_or_default());
    Some(format!("{}-{}-{}-{}", name, pkgver, pkgrel, arch))
}

fn run_lkpm(args: &[&str]) -> std::io::Result<std::process::ExitStatus> {
    let is_root = unsafe { libc::geteuid() } == 0;
    if is_root {
        Command::new("lkpm").args(args).status()
    } else {
        Command::new("sudo").arg("lkpm").args(args).status()
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() > 1 && (args[1] == "-h" || args[1] == "--help") {
        println!("");
        println!("---------------------------------------");
        println!("::: [ Liska Package Maker (1.0.0) ] :::");
        println!("---------------------------------------");
        println!("");
        println!("Usage: lkmake <command>");
        println!("> -d | --nodeps     skip dependency resolution and installation"); 
        println!("> -c | --clean      clean up work files after build");
        println!("> -i | --install    install package after successful build");
        println!("");
        exit(0);
    }
    let skip_deps = args.iter().any(|a| a == "-d" || a == "--nodeps");
    let install_after = args.iter().any(|a| a == "-i" || a == "--install");
    let clean_after = args.iter().any(|a| a == "-c" || a == "--clean");
    if !skip_deps {
        log("Resolving makedepends....");
        let deps = array_from_pkgbuild("makedepends");
        if !deps.is_empty() {
            resolve_and_install_deps(deps);
        } else { log_warn("No makedepends was found on PKGBUILD.") }
    }
    let srcdir = "src";
    let pkgdir = "pkg";
    let projectdir = "./";
    let fakeroot_state = ".fakeroot.state";
    let _ = download_sources(srcdir);
    let ok = check_integrity(srcdir);
    if !ok { 
        log_error("Integrity checks failed or skipped! Aborting....");
        exit(1);
    }
    extract_sources(srcdir);
    // Run PKGBUILD phases from project ./src folder so builds referencing the
    // top-level 'target' directory work as expected.
    fs::create_dir_all(srcdir).ok();
    fs::create_dir_all(pkgdir).ok();
    let cwd = projectdir;
    let _ = run_pkg_phase("prepare", cwd, srcdir, pkgdir, projectdir, "");
    let _ = run_pkg_phase("build", cwd, srcdir, pkgdir, projectdir, "");
    let _ = run_pkg_phase("check", cwd, srcdir, pkgdir, projectdir, "");
    let _ = run_pkg_phase("package", cwd, srcdir, pkgdir, projectdir, fakeroot_state);
    let pkgname = read_pkgname().unwrap_or_else(|| "package-unknown".to_string());
    write_pkginfo(pkgdir);
    write_buildinfo(pkgdir);
    let success = package_archive(pkgdir, &pkgname, projectdir, fakeroot_state);
    if success { log_success(&format!("Package successfully created: {}{}.lsk.tar.zst", projectdir, pkgname)); } else { log_error("Failed to create package tarball!"); }
    if install_after && success {
        let archive = format!("{}{}.lsk.tar.zst", projectdir, pkgname);
        let _ = run_lkpm(&["-ld", &archive, "--noconfirm"]);
    }
    if clean_after { 
        let _ = fs::remove_dir_all(srcdir); 
        let _ = fs::remove_dir_all(pkgdir);
        let _ = fs::remove_file(&fakeroot_state);
    }
}
