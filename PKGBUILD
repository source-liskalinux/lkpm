# PKGBUILD For lkpm, lkinit, lkchroot, and lkmake

# Contributor: Janorovic Volkov <janorovicvolkov@gmail.com>
# Maintainer: Janorovic Volkov <janorovicvolkov@gmail.com>

pkgname=lkpm
pkgver=1.1.0
pkgrel=1
pkgdesc="Liska Core Toolchain Ecosystem"
arch=('x86_64')
url="https://github.com/source-liskalinux/lkpm"
license=('GPL-3.0-or-later')
depends=(
    'ca-certificates' 'sqlite' 'zstd' 'xz' 'busybox' 'cpio' 'kmod' 'bash'
    'curl' 'tar' 'coreutils' 'fakeroot' 'gzip' 'bzip2' 'unzip'
    'p7zip' 'rustup' 'gcc' 'musl'
)
optdepends=(
    'git' 'make' 'clang' 'python' 'perl' 'ruby' 'nodejs'
    'npm' 'yarn' 'go'
)
makedepends=('rustup' 'pkgconf' 'sqlite')
backup=(
    'etc/lkpm.d/config.lua'
    'etc/lkpm.d/mirrorlist'
    'etc/lkinit.d/Cargo.toml'
    'etc/lkinit.d/init.rs'
)

build() {
    echo "--> [BUILD] Compiling lkpm, lkmake, lkinit, and lkchroot...."
    cargo build --release
}

check() {
    echo "-- [CHECK] Checking compiled binary...."
    cargo check --release --all-targets
}

package() {
    install -d "${pkgdir}/usr/bin"
    install -d "${pkgdir}/etc"
    echo "--> [PACKAGE] Installing lkpm...."
    install -Dm 755 "./target/release/lkpm" "${pkgdir}/usr/bin/lkpm"
    if [ -f "./etc/lkpm.d/config.lua" ]; then
        install -Dm 644 "./etc/lkpm.d/config.lua" "${pkgdir}/etc/lkpm.d/config.lua"
    fi
    if [ -f "./etc/lkpm.d/mirrorlist" ]; then
        install -Dm 644 "./etc/lkpm.d/mirrorlist" "${pkgdir}/etc/lkpm.d/mirrorlist"
    fi
    install -dm 700 "${pkgdir}/var/db/lkpm"
    chmod 700 "${pkgdir}/var/db/lkpm"
    chown root:root "${pkgdir}/usr/bin/lkpm"
    install -dm 755 "${pkgdir}/etc/lkpm.d/backup"
    chmod 755 "${pkgdir}/etc/lkpm.d/backup"
    chown root:root "${pkgdir}/etc/lkpm.d/backup"
    echo "--> [PACKAGE] Installing lkmake...."
    install -Dm 755 "./target/release/lkmake" "${pkgdir}/usr/bin/lkmake"
    echo "--> [PACKAGE] Installing lkinit...."
    install -Dm 755 "./target/release/lkinit" "${pkgdir}/usr/bin/lkinit"
    if [ -f "./etc/lkinit.d/init.rs" ]; then
        install -Dm 644 "./etc/lkinit.d/init.rs" "${pkgdir}/etc/lkinit.d/init.rs"
    fi
    if [ -f "./etc/lkinit.d/Cargo.toml" ]; then
        install -Dm 644 "./etc/lkinit.d/Cargo.toml" "${pkgdir}/etc/lkinit.d/Cargo.toml"
    fi
    echo "--> [PACKAGE] Installing lkchroot...."
    install -Dm 755 "./target/release/lkchroot" "${pkgdir}/usr/bin/lkchroot"
}
