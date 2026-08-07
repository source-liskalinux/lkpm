use lkpm::{cli, manager, ui};

use crate::cli::parse;

fn main() {
    match parse() {
        Ok(cmd) => {
            if let Err(e) = manager::handle(cmd) {
                ui::error(&e.to_string());
                std::process::exit(1);
            }
        }
        Err(e) => {
            ui::error(&e);
            std::process::exit(2);
        }
    }
}
