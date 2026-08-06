use colored::*;
use nix::mount::{mount, umount, MsFlags};
use std::ffi::CString;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::thread;
use std::time::Duration;

fn log(message: &str) { println!("{} {}", "[ i ]".bright_cyan(), message); }
fn success (message: &str) { println!("{} {}", "[ ✓ ]".bright_green(), message.bright_green()); }
fn warning (message: &str) { println!("{} {}", "[ ! ]".bright_yellow(), message.bright_yellow()); }
fn error (message: &str) { println!("{} {}", "[ ✗ ]".bright_red(), message.bright_red()); }

fn main() -> anyhow::Result<()> {
    println!("");
    println!("{}", "             ::: [ WELCOME TO LISKA LINUX ] :::".bright_cyan().bold());
    println!("");
    log("Mounting pseudo filesystems....");
    mount_pseudo_fs()?;
    success("Pseudo filesystems mounted successfully!");
    if IS_ISO {
        log("Scanning block devices for Liska ISO....");
        let bootmnt = "/run/liska/bootmnt";
        fs::create_dir_all(bootmnt).ok();
        fs::create_dir_all("/src_sfs").ok();
        fs::create_dir_all("/cow").ok();
        fs::create_dir_all("/new_root").ok();
        let mut found = false;
        for _ in 0..15 {
            let _ = Command::new("/bin/mdev").arg("-s").status();
            if let Ok(entries) = fs::read_dir("/dev") {
                for entry in entries.flatten() {
                    let path = entry.path();
                    let name = path.to_string_lossy();
                    if name.starts_with("/dev/sd") || name.starts_with("/dev/nvme") || name.starts_with("/dev/vd") || name.starts_with("/dev/sr") {
                        if mount(Some(path.as_path()), Path::new(bootmnt), None::<&str>, MsFlags::MS_RDONLY, None::<&str>).is_ok() {
                            if Path::new(&format!("{}/liskafs.sfs", bootmnt)).exists() {
                                found = true;
                                success(&format!("Found liskafs.sfs on {}!", name));
                                break;
                            }
                            let _ = umount(bootmnt);
                        }
                    }
                }
            }
            if found { break; }
            thread::sleep(Duration::from_millis(300));
        }
        if !found {
            error("CRITICAL: could not find liskafs.sfs!");
            warning("Initializing bash shell for emergency....");
            let _ = Command::new("/bin/sh").status();
            return Ok(());
        }
        log("Mounting SquashFS and setting up OverlayFS....");
        mount(Some(format!("{}/liskafs.sfs", bootmnt).as_str()), "/src_sfs", Some("squashfs"), MsFlags::MS_RDONLY, None::<&str>)?;
        mount(Some("tmpfs"), "/cow", Some("tmpfs"), MsFlags::empty(), None::<&str>)?;
        fs::create_dir_all("/cow/upper").ok();
        fs::create_dir_all("/cow/work").ok();
        mount(
            Some("overlay"),
            "/new_root",
            Some("overlay"),
            MsFlags::empty(),
            Some("lowerdir=/src_sfs,upperdir=/cow/upper,workdir=/cow/work"),
        )?;
        success("SquashFS and OverlayFS setup completed successfully!");
    } else {
        log("Loading storage and filesystem kernel modules....");
        load_essential_modules();
        log("Resolving root partition from cmdline....");
        let root_param = get_cmdline_param("root=");
        let real_dev = resolve_device(&root_param);
        log(&format!("Mounting root filesystem ({} -> {})....", real_dev.cyan(), "/new_root".cyan()));
        fs::create_dir_all("/new_root").ok();
        let mut mounted = false;
        for _ in 0..10 {
            let _ = Command::new("/bin/mdev").arg("-s").status();
            if mount(Some(real_dev.as_str()), "/new_root", None::<&str>, MsFlags::MS_RELATIME, None::<&str>).is_ok() 
               || mount(Some(real_dev.as_str()), "/new_root", None::<&str>, MsFlags::MS_RELATIME, Some("subvol=@")).is_ok() {
                mounted = true;
                break;
            }
            thread::sleep(Duration::from_millis(300));
        }
        if !mounted {
            error(&format!("CRITICAL: could not mount root filesystem {}!", real_dev.cyan()));
            warning("Initializing bash shell for emergency....");
            let _ = Command::new("/bin/sh").status();
            return Ok(());
        }
    }
    log(&format!("Moving virtual mounts into {}....", "/new_root".cyan()));
    move_virtual_mounts("/new_root")?;
    log("Searching rustysd for PID 1....");
    let rustysd_path = find_rustysd_binary("/new_root");
    success(&format!("Rustysd Found in {}! Initializing rustysd.", rustysd_path.cyan()));
    switch_root("/new_root", &rustysd_path)?;
    Ok(())
}

fn mount_pseudo_fs() -> anyhow::Result<()> {
    let _ = fs::create_dir_all("/proc");
    let _ = fs::create_dir_all("/sys");
    let _ = fs::create_dir_all("/dev");
    let _ = fs::create_dir_all("/run");
    let _ = mount(Some("proc"), "/proc", Some("proc"), MsFlags::empty(), None::<&str>);
    let _ = mount(Some("sysfs"), "/sys", Some("sysfs"), MsFlags::empty(), None::<&str>);
    let _ = mount(Some("devtmpfs"), "/dev", Some("devtmpfs"), MsFlags::empty(), None::<&str>);
    let _ = mount(Some("tmpfs"), "/run", Some("tmpfs"), MsFlags::empty(), None::<&str>);
    Ok(())
}

fn load_essential_modules() {
    let modules = [
        "ahci", "ata_piix", "libata", "sd_mod", "scsi_mod", 
        "virtio_blk", "virtio_pci", "nvme", "ext4", "btrfs", "xfs", "f2fs", "vfat", "overlay"
    ];
    for mod_name in modules {
        let _ = Command::new("/bin/modprobe").arg(mod_name).status();
    }
}

fn get_cmdline_param(param: &str) -> String {
    if let Ok(cmdline) = fs::read_to_string("/proc/cmdline") {
        for arg in cmdline.split_whitespace() {
            if arg.starts_with(param) {
                return arg.trim_start_matches(param).to_string();
            }
        }
    }
    String::new()
}

fn resolve_device(target: &str) -> String {
    if target.is_empty() { return String::new(); }
    if target.starts_with("UUID=") || target.starts_with("LABEL=") || target.starts_with("PARTUUID=") {
        if let Ok(output) = Command::new("blkid").output() {
            let out_str = String::from_utf8_lossy(&output.stdout);
            for line in out_str.lines() {
                if line.contains(target) {
                    if let Some(dev) = line.split(':').next() {
                        return dev.trim().to_string();
                    }
                }
            }
        }
    }
    target.to_string()
}

fn move_virtual_mounts(sysroot: &str) -> anyhow::Result<()> {
    for dir in &["dev", "proc", "sys", "run"] {
        let old_path = format!("/{}", dir);
        let new_path = format!("{}/{}", sysroot, dir);
        let _ = fs::create_dir_all(&new_path);
        let _ = mount(Some(old_path.as_str()), new_path.as_str(), None::<&str>, MsFlags::MS_MOVE, None::<&str>);
    }
    Ok(())
}

fn find_rustysd_binary(sysroot: &str) -> String {
    let candidates = [
        "/sbin/init",
        "/usr/bin/rustysd",
        "/usr/lib/systemd/systemd"
    ];
    for cand in candidates {
        if Path::new(&format!("{}{}", sysroot, cand)).exists() {
            return cand.to_string();
        }
    }
    "/sbin/init".to_string()
}

fn switch_root(sysroot: &str, init_path: &str) -> anyhow::Result<()> {
    std::env::set_current_dir(sysroot)?;
    nix::unistd::chroot(".")?;
    std::env::set_current_dir("/")?;
    let c_init = CString::new(init_path)?;
    let c_args = [c_init.clone()];
    nix::unistd::execv(&c_init, &c_args)?;
    Err(anyhow::anyhow!("Failed to execute PID 1 switch_root into rustysd!"))
}