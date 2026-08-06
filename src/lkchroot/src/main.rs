use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, exit};
use std::os::unix::process::CommandExt;
use colored::*;

pub fn info(msg: &str) { println!("{} {}", "[i]".bright_cyan(), msg); }
pub fn success(msg: &str) { println!("{} {}", "[✓]".bright_green(), msg.bright_green()); }
pub fn error(msg: &str) { eprintln!("{} {}", "[✗]".bright_red(), msg.bright_red()); }

fn require_root() {
    if unsafe { libc::geteuid() } != 0 {
        error("Root permission required. Use 'sudo' for this operation!");
        exit(1);
    }
}

struct MountGuard {
    mounts: Vec<PathBuf>,
}

impl MountGuard {
    fn new() -> Self {
        Self { mounts: Vec::new() }
    }
    fn mount(&mut self, source: &str, target: &Path, fstype: &str, flags: &[&str]) {
        fs::create_dir_all(target).ok();
        let mut args = vec!["-t", fstype];
        for flag in flags {
            args.push(flag);
        }
        args.push(source);
        args.push(target.to_str().unwrap());
        let res = Command::new("mount").args(&args).status();
        if let Ok(s) = res {
            if s.success() {
                self.mounts.push(target.to_path_buf());
            }
        }
    }
    fn mount_bind(&mut self, source: &Path, target: &Path) {
        fs::create_dir_all(target).ok();
        let res = Command::new("mount")
            .args(&["--bind", source.to_str().unwrap(), target.to_str().unwrap()])
            .status();
        if let Ok(s) = res {
            if s.success() {
                self.mounts.push(target.to_path_buf());
            }
        }
    }
}

impl Drop for MountGuard {
    fn drop(&mut self) {
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
    sync_host_configs(&target_dir);
    let mut guard = MountGuard::new();
    info("Mounting pseudo filesystems....");
    guard.mount("proc", &target_dir.join("proc"), "proc", &["-o", "nosuid,noexec,nodev"]);
    guard.mount("sysfs", &target_dir.join("sys"), "sysfs", &["-o", "nosuid,noexec,nodev"]);
    guard.mount("devtmpfs", &target_dir.join("dev"), "devtmpfs", &["-o", "mode=0755,nosuid"]);
    let dev_pts = target_dir.join("dev/pts");
    guard.mount("devpts", &dev_pts, "devpts", &["-o", "mode=0620,gid=5,nosuid,noexec"]);
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
        error(&format!("Failed to set working directory: {}", e));
        exit(1);
    }
    unsafe {
        let path_c = std::ffi::CString::new(target_dir.to_str().unwrap()).unwrap();
        if libc::chroot(path_c.as_ptr()) != 0 {
            error("Chroot syscall failed!");
            exit(1);
        }
    }
    unsafe {
       std::env::set_var("PATH", "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin");
       std::env::set_var("HOME", "/root");
    }
    let err = Command::new("sh")
        .args(&["-c", &custom_cmd])
        .exec();
    error(&format!("Failed to execute chroot command: {}.", err));
}
