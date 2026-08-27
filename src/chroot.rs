use crate::ui::{error, info, warning};
use anyhow::{anyhow, Context, Result};
use std::ffi::CString;
use std::fs;
use std::io;
use std::os::raw::c_int;
use std::path::{Path, PathBuf};
use std::process::Command;

fn isolate_mount_namespace() {
    let unshared = unsafe { libc::unshare(libc::CLONE_NEWNS) == 0 };
    if !unshared {
        warning("Cannot unshare mount namespace! Are you in an unprivileged container?");
        return;
    }
    let made_private = Command::new("mount")
        .args(["--make-rprivate", "/"])
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
        args.extend_from_slice(flags);
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
            .args(["--bind", source.to_str().unwrap(), target_str])
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
        for target in self.mounts.iter().rev() {
            let _ = Command::new("umount").args(["-l", target.to_str().unwrap()]).status();
        }
    }
}

fn sync_host_configs(target: &Path) {
    let target_etc = target.join("etc");
    fs::create_dir_all(&target_etc).ok();
    let host_resolv = Path::new("/etc/resolv.conf");
    if host_resolv.exists() {
        let _ = fs::copy(host_resolv, target_etc.join("resolv.conf"));
    }
}

fn mount_pseudo_filesystems(target: &Path) -> Result<MountGuard> {
    let mut guard = MountGuard::new();
    let mut ok = true;
    ok &= guard.mount("proc", &target.join("proc"), "proc", &["-o", "nosuid,noexec,nodev"]);
    ok &= guard.mount("sysfs", &target.join("sys"), "sysfs", &["-o", "nosuid,noexec,nodev"]);
    ok &= guard.mount("devtmpfs", &target.join("dev"), "devtmpfs", &["-o", "mode=0755,nosuid"]);
    ok &= guard.mount("devpts", &target.join("dev/pts"), "devpts", &["-o", "mode=0620,gid=5,nosuid,noexec"]);
    ok &= guard.mount("shm", &target.join("dev/shm"), "tmpfs", &["-o", "mode=1777,nosuid,nodev"]);
    if !ok {
        return Err(anyhow!("Failed to set up one or more required chroot mounts"));
    }
    let run_dir = Path::new("/run");
    if run_dir.exists() {
        guard.mount_bind(run_dir, &target.join("run"));
    }
    Ok(guard)
}

fn link_busybox_sh(busybox: &Path) -> Result<()> {
    let bin_dir = busybox.parent().unwrap();
    let sh_link = bin_dir.join("sh");
    let _ = fs::remove_file(&sh_link);
    let bash_link = bin_dir.join("bash");
    let _ = fs::remove_file(&bash_link);
    let sh_status = Command::new("ln")
        .args(["-sf", "busybox", sh_link.to_str().unwrap()])
        .status()
        .context("Cannot create sh symlink")?;
    if !sh_status.success() {
        return Err(anyhow!("Cannot create sh ➔ busybox symlink"));
    }
    Ok(())
}

fn ensure_posix_shell(target: &Path) -> Result<()> {
    let real_shells = ["usr/bin/bash", "bin/bash", "usr/bin/zsh", "bin/zsh", "usr/bin/sh", "bin/sh"];
    if real_shells.iter().any(|c| target.join(c).exists()) {
        return Ok(());
    }
    if let Some(busybox) = ["usr/bin/busybox", "bin/busybox"]
        .iter()
        .map(|p| target.join(p))
        .find(|p| p.exists())
    {
        info("No shell found in target but busybox is present! Linking sh to busybox....");
        return link_busybox_sh(&busybox);
    }
    let Some(host_busybox) = ["/usr/bin/busybox", "/bin/busybox"]
        .iter()
        .map(PathBuf::from)
        .find(|p| p.exists())
    else {
        return Err(anyhow!(
            "Target has no shell and busybox, the host has no busybox to fall back to! Install a base system or busybox into the target first!"
        ));
    };
    warning("Target has no shell at all! Copying busybox from the host....");
    let target_bin = target.join("usr/bin");
    fs::create_dir_all(&target_bin).with_context(|| format!("Cannot create {}", target_bin.display()))?;
    let target_busybox = target_bin.join("busybox");
    fs::copy(&host_busybox, &target_busybox).context("Cannot copy busybox into target")?;
    let _ = Command::new("chmod").args(["+x", target_busybox.to_str().unwrap()]).status();
    link_busybox_sh(&target_busybox)
}

fn resolve_shared_libs(binary: &Path) -> Vec<PathBuf> {
    let output = match Command::new("ldd").arg(binary).output() {
        Ok(o) => o,
        Err(_) => return Vec::new(),
    };
    if !output.status.success() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut libs = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.contains("linux-vdso.so") {
            continue;
        }
        let path_part = line.split_once("=>").map(|(_, rhs)| rhs).unwrap_or(line);
        if let Some(path_str) = path_part.split_whitespace().next() {
            if path_str.starts_with('/') {
                libs.push(PathBuf::from(path_str));
            }
        }
    }
    libs
}

fn copy_into_target(target: &Path, host_path: &Path) -> Result<()> {
    let rel = host_path.strip_prefix("/").unwrap_or(host_path);
    let dest = target.join(rel);
    if dest.exists() {
        return Ok(());
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).with_context(|| format!("Cannot create {}", parent.display()))?;
    }
    fs::copy(host_path, &dest)
        .with_context(|| format!("Cannot copy {} into target", host_path.display()))?;
    let _ = Command::new("chmod").args(["+x", dest.to_str().unwrap()]).status();
    Ok(())
}

fn ensure_bash(target: &Path) -> Result<&'static str> {
    for candidate in ["usr/bin/bash", "bin/bash"] {
        if target.join(candidate).exists() {
            return Ok("/usr/bin/bash");
        }
    }
    let Some(host_bash) = ["/usr/bin/bash", "/bin/bash"]
        .iter()
        .map(PathBuf::from)
        .find(|p| p.exists())
    else {
        return Err(anyhow!("Target and host has no bash at all!"));
    };
    warning("Target has no bash at all! Copying bash and its shared libraries from the host....");
    let target_bin = target.join("usr/bin");
    fs::create_dir_all(&target_bin).with_context(|| format!("Cannot create {}", target_bin.display()))?;
    let target_bash = target_bin.join("bash");
    fs::copy(&host_bash, &target_bash).context("Cannot copy bash into target")?;
    let _ = Command::new("chmod").args(["+x", target_bash.to_str().unwrap()]).status();
    for lib in resolve_shared_libs(&host_bash) {
        copy_into_target(target, &lib)?;
    }
    Ok("/usr/bin/bash")
}

fn find_interactive_shell(target: &Path) -> String {
    let shells = ["/usr/bin/zsh", "/bin/zsh", "/usr/bin/bash", "/bin/bash", "/usr/bin/sh", "/bin/sh"];
    for sh in shells {
        if target.join(sh.trim_start_matches('/')).exists() {
            return sh.to_string();
        }
    }
    "/bin/sh".to_string()
}

fn fork_exec_in_chroot(target_dir: &Path, shell_bin: &str, command: &str) -> Result<i32> {
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err(anyhow!("Cannot fork: {}", io::Error::last_os_error()));
    }
    if pid == 0 {
        if let Err(e) = std::env::set_current_dir(target_dir) {
            error(&format!("Failed to set working directory: {e}"));
            std::process::exit(126);
        }
        unsafe {
            let path_c = CString::new(target_dir.to_str().unwrap()).unwrap();
            if libc::chroot(path_c.as_ptr()) != 0 {
                error("Chroot syscall failed!");
                std::process::exit(126);
            }
            let root_c = CString::new("/").unwrap();
            if libc::chdir(root_c.as_ptr()) != 0 {
                error("Cannot chdir to / after chroot!");
                std::process::exit(126);
            }
        }
        unsafe {
            std::env::set_var("PATH", "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin");
            std::env::set_var("HOME", "/root");
        }
        use std::os::unix::process::CommandExt;
        let err = Command::new(shell_bin).args(["-c", command]).exec();
        error(&format!("Failed to execute chroot command: {err}."));
        std::process::exit(126);
    }
    let mut status: c_int = 0;
    loop {
        let r = unsafe { libc::waitpid(pid, &mut status, 0) };
        if r == -1 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(anyhow!("waitpid failed: {e}"));
        }
        break;
    }
    if libc::WIFEXITED(status) {
        Ok(libc::WEXITSTATUS(status))
    } else if libc::WIFSIGNALED(status) {
        Ok(128 + libc::WTERMSIG(status))
    } else {
        Ok(1)
    }
}

pub enum Shell {
    Auto,
    Posix(String),
    Bash(String),
}

pub fn run_in_chroot(target_dir: &Path, shell: Shell) -> Result<i32> {
    isolate_mount_namespace();
    sync_host_configs(target_dir);
    let guard = mount_pseudo_filesystems(target_dir)?;
    let (shell_bin, command) = match &shell {
        Shell::Auto => {
            ensure_posix_shell(target_dir)?;
            ("sh".to_string(), find_interactive_shell(target_dir))
        }
        Shell::Posix(cmd) => {
            ensure_posix_shell(target_dir)?;
            ("sh".to_string(), cmd.clone())
        }
        Shell::Bash(cmd) => {
            let bash = ensure_bash(target_dir)?;
            (bash.to_string(), cmd.clone())
        }
    };
    let result = fork_exec_in_chroot(target_dir, &shell_bin, &command);
    drop(guard);
    result
}