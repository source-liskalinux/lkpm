use std::env;
use std::fs as sfs;
use std::path::{Path, PathBuf};
use std::process::{Command, exit, Stdio};
use std::io::{Write, Read};
use indicatif::{ProgressBar, ProgressStyle};
use lkpm::ui::{info, success, error};
use liblk::fs;

fn require_root() {
    if unsafe { libc::getuid() } != 0 {
        error("Operation not permitted (os error 1)!");
        exit(1);
    }
}

fn run_command(cmd: &str, args: &[&str]) -> Result<(), String> {
    let status = Command::new(cmd)
        .args(args)
        .status()
        .map_err(|err| format!("Could not start {}: {}", cmd, err))?;
    if status.success() { Ok(()) } else { Err(format!("Command {} failed to run!", cmd)) }
}

fn pack_initramfs_with_progress(temp_ramdisk: &Path, output_img: &Path) -> Result<(), String> {
    let mut paths = Vec::new();
    let mut total_size = 0u64;
    fn collect_files(dir: &Path, base: &Path, paths: &mut Vec<String>, total_size: &mut u64) {
        if let Ok(entries) = sfs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if let Ok(sym_meta) = path.symlink_metadata() {
                    if let Ok(rel) = path.strip_prefix(base) {
                        paths.push(rel.display().to_string());
                    }
                    if sym_meta.is_file() {
                        *total_size += sym_meta.len();
                    } else if sym_meta.is_dir() {
                        collect_files(&path, base, paths, total_size);
                    }
                }
            }
        }
    }
    collect_files(temp_ramdisk, temp_ramdisk, &mut paths, &mut total_size);
    if total_size == 0 { total_size = 1; }
    let pb = ProgressBar::new(total_size);
    pb.set_style(
        ProgressStyle::with_template(
            "{prefix:.bold} [{elapsed_precise}] [{bar:75.bright.cyan/blue}] {percent}% | {bytes}/{total_bytes} ({eta})"
        )
        .unwrap()
        .progress_chars("#•-")
    );
    pb.set_prefix("Packing initramfs:");
    let mut cpio = Command::new("cpio")
        .args(&["-H", "newc", "-o", "--quiet"])
        .current_dir(temp_ramdisk)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("Failed to run cpio: {}", e))?;
    let mut zstd = Command::new("zstd")
        .args(&["-19", "-T0", "-q", "-f", "-o", output_img.to_str().unwrap()])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("Failed to run zstd: {}", e))?;
    let mut cpio_stdin = cpio.stdin.take().ok_or("Failed to open stdin cpio!")?;
    std::thread::spawn(move || {
        for path in paths {
            let _ = writeln!(cpio_stdin, "{}", path);
        }
    });
    let mut cpio_stdout = cpio.stdout.take().ok_or("Failed to open stdout cpio!")?;
    let mut zstd_stdin = zstd.stdin.take().ok_or("Failed to open stdin zstd!")?;
    let mut buffer = [0u8; 16 * 1024];
    loop {
        let n = cpio_stdout.read(&mut buffer).map_err(|e| e.to_string())?;
        if n == 0 { break; }
        zstd_stdin.write_all(&buffer[..n]).map_err(|e| e.to_string())?;
        pb.inc(n as u64);
    }
    drop(zstd_stdin);
    cpio.wait().ok();
    zstd.wait().ok();
    pb.finish_with_message("Initramfs has been packed!");
    Ok(())
}

fn compile_init_template(rootfs: &Path, cache_dir: &Path, target_init_bin: &Path) -> Result<(), String> {
    info("Compiling /etc/lkinit.d/init.rs....");
    let template_rootfs = rootfs.join("etc/lkinit.d/init.rs");
    let cargo_rootfs = rootfs.join("etc/lkinit.d/Cargo.toml");
    let (template_path, cargo_path) = if template_rootfs.exists() && cargo_rootfs.exists() {
        (template_rootfs, cargo_rootfs)
    } else {
        (
            PathBuf::from("/etc/lkinit.d/init.rs"),
            PathBuf::from("/etc/lkinit.d/Cargo.toml"),
        )
    };
    if !template_path.exists() || !cargo_path.exists() {
        return Err("CRITICAL: Init template (init.rs) or Cargo.toml not found!".into());
    }
    let build_dir = cache_dir.join("init");
    fs::lkremove(&build_dir).ok();
    fs::lkcreate(&build_dir.join("src")).map_err(|e| e.to_string())?;
    fs::lkcopy(&template_path, &build_dir.join("src/main.rs"))
        .map_err(|e| format!("Failed to copy init.rs template: {}", e))?;
    fs::lkcopy(&cargo_path, &build_dir.join("Cargo.toml"))
        .map_err(|e| format!("Failed to copy Cargo.toml template: {}", e))?;
    let _ = Command::new("rustup")
        .env("RUSTUP_TOOLCHAIN", "stable")
        .args(&["default", "stable"])
        .status();
    let _ = Command::new("rustup")
        .env("RUSTUP_TOOLCHAIN", "stable")
        .args(&["target", "add", "x86_64-unknown-linux-musl"])
        .status();
    let cargo_default = Command::new("cargo")
        .env("RUSTUP_TOOLCHAIN", "stable")
        .env("RUSTFLAGS", "-C target-feature=+crt-static")
        .args(&[
            "build",
            "--manifest-path", &build_dir.join("Cargo.toml").display().to_string(),
            "--release",
            "--target", "x86_64-unknown-linux-musl"
        ])
        .status()
        .map_err(|e| format!("Cargo compilation error: {}", e))?;
    if !cargo_default.success() {
        return Err("Failed to compile /etc/lkinit.d/init.rs!".into());
    }
    let compiled_binary = build_dir.join("target/x86_64-unknown-linux-musl/release/init");
    fs::lkcopy(&compiled_binary, &target_init_bin)
        .map_err(|e| format!("Failed to copy compiled binary to init: {}", e))?;
    fs::lkremove(&build_dir).ok();
    Ok(())
}

pub fn generate_liska_initramfs(rootfs: &Path, cache_dir: &Path, output_img: &Path) -> Result<(), String> {
    info("Starting to generate initramfs-liska.img....");
    let rootfs_mod_dir = rootfs.join("usr/lib/modules");
    let mut kernel_version = String::new();
    if rootfs_mod_dir.exists() {
        if let Ok(entries) = sfs::read_dir(&rootfs_mod_dir) {
            for entry in entries.flatten() {
                if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    kernel_version = entry.file_name().to_string_lossy().into_owned();
                    break;
                }
            }
        }
    }
    if kernel_version.is_empty() {
        return Err("FATAL: kernel version not found!".into());
    }
    let temp_ramdisk = cache_dir.join("ramdisk");
    fs::lkremove(&temp_ramdisk).ok();
    let dirs = &[
        "dev", "proc", "sys", "root", "run", "etc",
        "usr/bin", "usr/lib", "usr/lib/modules"
    ];
    for dir in dirs {
        fs::lkcreate(&temp_ramdisk.join(dir)).ok();
    }
    fs::lksymlink(&PathBuf::from("usr/bin"), &temp_ramdisk.join("bin")).ok();
    fs::lksymlink(&PathBuf::from("usr/bin"), &temp_ramdisk.join("sbin")).ok();
    fs::lksymlink(&PathBuf::from("usr/bin"), &temp_ramdisk.join("usr/sbin")).ok();
    fs::lksymlink(&PathBuf::from("usr/lib"), &temp_ramdisk.join("lib")).ok();
    fs::lksymlink(&PathBuf::from("usr/lib"), &temp_ramdisk.join("lib64")).ok();
    let dst_mod_dir = temp_ramdisk.join("usr/lib/modules");
    let _ = run_command("cp", &["-ax", rootfs_mod_dir.to_str().unwrap(), temp_ramdisk.join("usr/lib/").to_str().unwrap()]);
    info("Uncompressing kernel modules....");
    let target_kernel_dir = dst_mod_dir.join(&kernel_version);
    let _ = run_command("sh", &["-c", &format!("find {} -name '*.ko.zst' -exec unzstd -q --rm -f {{}} \\;", target_kernel_dir.display())]);
    info("Regenerating module dependency index....");
    let _ = run_command("depmod", &["-a", "-b", temp_ramdisk.to_str().unwrap(), &kernel_version]);
    let busybox_path = rootfs.join("usr/bin/busybox");
    let busybox_src = if busybox_path.exists() { busybox_path } else { rootfs.join("bin/busybox") };
    if busybox_src.exists() {
        info("Copying busybox to initramfs....");
        fs::lkcopy(&busybox_src, &temp_ramdisk.join("usr/bin/busybox")).ok();
        let busybox_links = &[
            "sh", "bash", "cttyhack", "mount", "umount", "mdev", 
            "insmod", "modprobe", "blkid", "losetup", "mknod",
            "ls", "cat", "echo", "clear", "mkdir", "rm", "cp", 
            "mv", "reboot", "poweroff", "which"
        ];
        for link in busybox_links {
            let link_path = temp_ramdisk.join("usr/bin").join(link);
            let _ = fs::lkremove(&link_path);
            let _ = fs::lksymlink(&PathBuf::from("busybox"), &link_path);
            let _ = fs::lkpermissions(&link_path, "+x");
        }
    }
    let liska_libs = &[
        "ld-linux-x86-64.so.2",
        "libc.so.6",
        "libm.so.6",
        "libresolv.so.2",
    ];
    info("Copying essential shared libraries to initramfs....");
    for lib in liska_libs {
        let src_lib_path = rootfs.join("usr/lib").join(lib);
        if src_lib_path.exists() {
            let target_dest = temp_ramdisk.join("usr/lib/");
            let _ = run_command("cp", &["-L", src_lib_path.to_str().unwrap(), target_dest.to_str().unwrap()]);
        }
    }
    let init_path = temp_ramdisk.join("init");
    compile_init_template(rootfs, &cache_dir, &init_path)?;
    fs::lkpermissions(&init_path, "+x")?;
    if let Some(parent) = output_img.parent() {
        fs::lkcreate(&parent).ok();
    }
    info("Packing initramfs image....");
    pack_initramfs_with_progress(&temp_ramdisk, output_img)?;
    fs::lkremove(&temp_ramdisk).ok();
    success("Initramfs generated successfully!");
    Ok(())
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() > 1 && (args[1] == "--help" || args[1] == "-h") {
        println!("");
        println!("-----------------------------------");
        println!("::: [ Liska Initramfs (1.0.0) ] :::");
        println!("-----------------------------------");
        println!("Usage: lkinit <command> [target (optional for --root and --output)]");
        println!("> --root <path>            specify the root filesystem path (default: /)");
        println!("> --output <path>          specify the output initramfs image path (default: /boot/initramfs-liska.img)");
        println!("> /etc/lkinit.d/init.rs    default init template path");
        println!("");
        exit(0);
    }
    require_root();
    let mut rootfs = PathBuf::from("/");
    let mut output = PathBuf::from("/boot/initramfs-liska.img");
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--root" => {
                if i + 1 < args.len() { rootfs = PathBuf::from(&args[i + 1]); i += 1; }
            }
            "--output" => {
                if i + 1 < args.len() { output = PathBuf::from(&args[i + 1]); i += 1; }
            }
            _ => {}
        }
        i += 1;
    }
    let cache_dir = rootfs.join("var/cache/lkinit");
    fs::lkcreate(&cache_dir).ok();
    fs::lkpermissions(&cache_dir, &"700".to_string()).ok();
    if let Err(e) = generate_liska_initramfs(&rootfs, &cache_dir, &output) {
        error(&format!("CRITICAL: {}", e));
        exit(1);
    }
}
