use lkpm::chroot::{run_in_chroot, Shell};
use lkpm::ui::error;
use std::env;
use std::path::PathBuf;
use std::process::exit;

fn require_root() {
    if unsafe { libc::getuid() } != 0 {
        error("Operation not permitted (os error 1)!");
        exit(1);
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 || args[1] == "--help" {
        println!("");
        println!("--------------------------------");
        println!("::: [ Liska Chroot (1.0.0) ] :::");
        println!("--------------------------------");
        println!("");
        println!("Usage: lkchroot <target-dir> [command (optional)]");
        println!("");
        exit(0);
    }
    require_root();
    let target_dir = PathBuf::from(&args[1]).canonicalize().unwrap_or_else(|_| {
        error("Invalid target directory!");
        exit(1);
    });
    let shell = if args.len() > 2 {
        Shell::Posix(args[2..].join(" "))
    } else {
        Shell::Auto
    };
    match run_in_chroot(&target_dir, shell) {
        Ok(code) => exit(code),
        Err(e) => {
            error(&format!("{e:#}"));
            exit(1);
        }
    }
}