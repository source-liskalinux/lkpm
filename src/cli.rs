use std::env;
use std::path::PathBuf;

#[derive(Debug)]
pub enum Command {
    Install {
        packages: Vec<String>,
        install_deps: bool,
        local: bool,
        noconfirm: bool,
        root: Option<PathBuf>,
    },
    Update {
        packages: Vec<String>,
        noconfirm: bool,
        root: Option<PathBuf>,
    },
    Delete {
        packages: Vec<String>,
        noconfirm: bool,
        root: Option<PathBuf>,
    },
    Refresh {
        root: Option<PathBuf>,
    },
    UpdateRefresh {
        packages: Vec<String>,
        noconfirm: bool,
        root: Option<PathBuf>,
    },
    Package {
        package: String,
        root: Option<PathBuf>,
    },
    Help,
}

pub fn parse() -> Result<Command, String> {
    let args: Vec<String> = env::args().skip(1).collect();
    parse_args(args)
}

fn parse_args(args: Vec<String>) -> Result<Command, String> {
    if args.is_empty() {
        return Ok(Command::Help);
    }
    let mut noconfirm = false;
    let mut root = None;
    let mut filtered = Vec::new();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        if arg == "--noconfirm" {
            noconfirm = true;
        } else if let Some(value) = arg.strip_prefix("--root=") {
            if value.is_empty() {
                return Err("--root argument requires a path!".into());
            }
            root = Some(PathBuf::from(value));
        } else if arg == "--root" {
            let value = args
                .next()
                .ok_or_else(|| "--root argument requires a path!".to_string())?;
            root = Some(PathBuf::from(value));
        } else {
            filtered.push(arg);
        }
    }
    if filtered.is_empty() {
        return Ok(Command::Help);
    }
    match filtered[0].as_str() {
        "-i" => {
            if filtered.len() < 2 {
                return Err("-i argument requires at least one package!".into());
            }
            Ok(Command::Install {
                packages: filtered[1..].to_vec(),
                install_deps: false,
                local: false,
                noconfirm,
                root,
            })
        }
        "-id" | "-di" => {
            if filtered.len() < 2 {
                return Err("-id or -di argument requires at least one package!".into());
            }
            Ok(Command::Install {
                packages: filtered[1..].to_vec(),
                install_deps: true,
                local: false,
                noconfirm,
                root,
            })
        }
        "-l" => {
            if filtered.len() < 2 {
                return Err("-l argument requires at least one local package file!".into());
            }
            Ok(Command::Install {
                packages: filtered[1..].to_vec(),
                install_deps: false,
                local: true,
                noconfirm,
                root,
            })
        }
        "-ld" | "-dl" => {
            if filtered.len() < 2 {
                return Err("-ld or -dl argument requires at least one local package file!".into());
            }
            Ok(Command::Install {
                packages: filtered[1..].to_vec(),
                install_deps: true,
                local: true,
                noconfirm,
                root,
            })
        }
        "-u" => {
            let packages = if filtered.len() >= 2 {
                filtered[1..].to_vec()
            } else {
                Vec::new()
            };
            Ok(Command::Update {
                packages,
                noconfirm,
                root,
            })
        }
        "-d" => {
            if filtered.len() < 2 {
                return Err("-d argument requires at least one package!".into());
            }
            Ok(Command::Delete {
                packages: filtered[1..].to_vec(),
                noconfirm,
                root,
            })
        }
        "-r" => Ok(Command::Refresh { root }),
        "-ru" | "-ur" => Ok(Command::UpdateRefresh {
            packages: filtered[1..].to_vec(),
            noconfirm,
            root,
        }),
        "-p" => {
            if filtered.len() != 2 {
                return Err("-p argument requires a package!".into());
            }
            Ok(Command::Package {
                package: filtered[1].clone(),
                root,
            })
        }
        "--help" => Ok(Command::Help),
        _ => Err(format!("Unknown option {}", filtered[0])),
    }
}

