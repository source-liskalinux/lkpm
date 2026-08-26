use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, exit};
use colored::*;

fn info(msg: &str) { println!("{} {}", "[i]".bright_cyan(), msg); }
fn success(msg: &str) { println!("{} {}", "[✓]".bright_green(), msg.bright_green()); }
fn warning(msg: &str) { println!("{} {}", "[!]".bright_yellow(), msg.bright_yellow()); }
fn error(msg: &str) { eprintln!("{} {}", "[✗]".bright_red(), msg.bright_red()); }

fn require_root() {
    if unsafe { libc::getuid() } != 0 {
        error("Operation not permitted (os error 1)!");
        exit(1);
    }
}

fn isolate_mount_namespace() {
    let unshared = unsafe { libc::unshare(libc::CLONE_NEWNS) == 0 };
    if !unshared {
        warning("Cannot unshare mount namespace! Are you in an unprivileged container?");
        return;
    }
    let made_private = Command::new("mount")
        .args(&["--make-rprivate", "/"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !made_private {
        warning("Cannot make mount namespace private! Mounts may not be fully isolated.");
    }
}

struct MountGuard {
    mounts: Vec<PathBuf>,
}

impl MountGuard {
    fn new() -> Self {
        Self { mounts: Vec::new() }
    }
    fn mount(&mut self, source: &str, target: &Path, fstype: &str, flags: &[&str]) -> bool {
        if let Err(e) = fs::create_dir_all(target) {
            error(&format!("Cannot create {}: {e}", target.display()));
            return false;
        }
        let mut args = vec!["-t", fstype];
        for flag in flags {
            args.push(flag);
        }
        args.push(source);
        let target_str = target.to_str().unwrap();
        args.push(target_str);
        match Command::new("mount").args(&args).status() {
            Ok(s) if s.success() => {
                self.mounts.push(target.to_path_buf());
                true
            }
            Ok(s) => {
                error(&format!("Cannot mount {fstype} on {target_str} (exit {:?})", s.code()));
                false
            }
            Err(e) => {
                error(&format!("Cannot run mount for {target_str}: {e}"));
                false
            }
        }
    }
    fn mount_bind(&mut self, source: &Path, target: &Path) -> bool {
        if let Err(e) = fs::create_dir_all(target) {
            error(&format!("Cannot create {}: {e}", target.display()));
            return false;
        }
        let target_str = target.to_str().unwrap();
        match Command::new("mount")
            .args(&["--bind", source.to_str().unwrap(), target_str])
            .status()
        {
            Ok(s) if s.success() => {
                self.mounts.push(target.to_path_buf());
                true
            }
            Ok(s) => {
                error(&format!("Cannot bind-mount {} on {target_str} (exit {:?})", source.display(), s.code()));
                false
            }
            Err(e) => {
                error(&format!("Cannot run mount for {target_str}: {e}"));
                false
            }
        }
    }
}

impl Drop for MountGuard {
    fn drop(&mut self) {
        if self.mounts.is_empty() {
            return;
        }
        info("Cleaning up chroot mountpoints....");
        for target in self.mounts.iter().rev() {
            let _ = Command::new("umount")
                .args(&["-l", target.to_str().unwrap()])
                .status();
        }
        success("Unmounted all pseudo filesystems successfully!");
    }
}

fn sync_host_configs(target: &Path) {
    info("Syncing host configuration files into chroot environment....");
    let target_etc = target.join("etc");
    fs::create_dir_all(&target_etc).ok();
    let host_resolv = Path::new("/etc/resolv.conf");
    if host_resolv.exists() {
        let _ = fs::copy(host_resolv, target_etc.join("resolv.conf"));
    }
}

fn find_shell(target: &Path) -> String {
    let shells = ["/usr/bin/zsh", "/bin/zsh", "/usr/bin/bash", "/bin/bash", "/bin/sh"];
    for sh in shells {
        if target.join(sh.trim_start_matches('/')).exists() {
            return sh.to_string();
        }
    }
    "/bin/sh".to_string()
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 || args[1] == "--help" {
        println!("");
        println!("--------------------------------");
        println!("::: [ Liska Chroot (1.0.0) ] :::");
        println!("--------------------------------");
        println!("");
        println!("Usage: lkchroot <target-directory> [command (optional)]");
        println!("");
        exit(0);
    }
    require_root();
    let target_dir = PathBuf::from(&args[1]).canonicalize().unwrap_or_else(|_| {
        error("Invalid target directory!");
        exit(1);
    });
    isolate_mount_namespace();
    sync_host_configs(&target_dir);
    let mut guard = MountGuard::new();
    info("Mounting pseudo filesystems....");
    let mut critical_ok = true;
    critical_ok &= guard.mount("proc", &target_dir.join("proc"), "proc", &["-o", "nosuid,noexec,nodev"]);
    critical_ok &= guard.mount("sysfs", &target_dir.join("sys"), "sysfs", &["-o", "nosuid,noexec,nodev"]);
    critical_ok &= guard.mount("devtmpfs", &target_dir.join("dev"), "devtmpfs", &["-o", "mode=0755,nosuid"]);
    let dev_pts = target_dir.join("dev/pts");
    critical_ok &= guard.mount("devpts", &dev_pts, "devpts", &["-o", "mode=0620,gid=5,nosuid,noexec"]);
    let dev_shm = target_dir.join("dev/shm");
    critical_ok &= guard.mount("shm", &dev_shm, "tmpfs", &["-o", "mode=1777,nosuid,nodev"]);
    if !critical_ok {
        error("Failed to set up one or more required chroot mounts! Aborting the operation!");
        drop(guard);
        exit(1);
    }
    let run_dir = Path::new("/run");
    if run_dir.exists() {
        guard.mount_bind(run_dir, &target_dir.join("run"));
    }
    let custom_cmd = if args.len() > 2 {
        args[2..].join(" ")
    } else {
        find_shell(&target_dir)
    };
    success(&format!("Pseudo filesystems has been mounted! Entering {} chroot environment....", target_dir.display()));
    if let Err(e) = std::env::set_current_dir(&target_dir) {
        error(&format!("Failed to set working directory: {e}"));
        drop(guard);
        exit(1);
    }
    unsafe {
        let path_c = std::ffi::CString::new(target_dir.to_str().unwrap()).unwrap();
        if libc::chroot(path_c.as_ptr()) != 0 {
            error("Chroot syscall failed!");
            drop(guard);
            exit(1);
        }
    }
    unsafe {
       std::env::set_var("PATH", "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin");
       std::env::set_var("HOME", "/root");
    }
    let status = Command::new("sh").args(["-c", &custom_cmd]).status();
    drop(guard);
    match status {
        Ok(s) => exit(s.code().unwrap_or(1)),
        Err(e) => {
            error(&format!("Failed to execute chroot command: {e}."));
            exit(1);
        }
    }
}