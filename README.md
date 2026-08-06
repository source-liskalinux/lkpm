# Liska Core Toolchain Ecosystem
A small Rust-based Liska Linux tool source code repository.

## Overview
This repository contains the `lkpm`, `lkmake`, `lkinit`, and `lkchroot` CLI written in Rust. The following instructions explain how to contribute, build, test, and create a binary package using a `PKGBUILD` file.

## Contributing
- Fork the repository and create a topic branch: `git checkout -b feat/your-feature`.
- Keep commits focused and write clear messages.
- Add tests for new behaviors and run the test suite locally before submitting a pull request.

Recommended local workflow:
```bash
git clone https://github.com/source-liskalinux/lkpm.git
git remote add upstream https://github.com/source-liskalinux/lkpm.git
git fetch upstream
git checkout -b feat/your-feature
# make changes, run tests, format
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test
```

Create a pull request against the main branch when ready and include a short description of the change and any migration notes.

## Development: building and running
Prerequisites:
- Rust toolchain
- `cargo` available on PATH

Build in release mode:
```bash
cargo build --release
```

Run directly with cargo (for quick iteration):
```bash
cargo run -- <args>
```

Or run the compiled binary:
```bash
./target/release/lkpm <args>
```

## Testing and code quality
- Run tests:
```bash
cargo test
```

- Format code:
```bash
cargo fmt --all
```

- Lint with clippy:
```bash
cargo clippy --all-targets -- -D warnings
```

## Packaging with PKGBUILD
A `PKGBUILD` is included to build a distributable package. Verify or update the `pkgname`, `pkgver`, `pkgrel`, `source`, `license`, and the `build()` / `package()` functions as needed.

Typical `PKGBUILD` build steps for a Rust project:
```bash
# build the package file (requires lkmake or makepkg),
# the package will be compressed to .tar.zst tarball.
#
# example:
# > if with lkmake (Liska Linux):
lkmake
# > if with makepkg (e.g. Arch Linux, Manjaro, etc...):
makepkg -s
```

Notes:
- The repository already contains a `PKGBUILD`, update it to match the desired `pkgver` and source layout.
- `lkmake` will produce a package archive (e.g. `lkpm-x.x.x-1-x86_64.lsk.tar.zst`) in the working directory.

## Installing locally from the built binary
You can install the binary locally with `lkmake` (or `makepkg` if you use Arch based) without `cargo build --release`:
```bash
# example:
# > if with lkmake (Liska Linux):
lkmake -i
# > if with makepkg (e.g. Arch Linux, Manjaro, etc...):
makepkg -si
```

## License
See the `LICENSE` file in the repository for licensing details.

## Maintainers
File issues and pull requests on the repository tracker. Include logs and steps to reproduce when reporting bugs.
