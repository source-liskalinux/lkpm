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

build() {
    echo "--> [BUILD] Compiling lkpm, lkmake, lkinit, and lkchroot...."
    cargo build --release
}

package() {
    install -d "${pkgdir}/usr/bin"
    install -d "${pkgdir}/etc"
    echo "--> [PACKAGE] Installing lkpm...."
    install -Dm755 "./target/release/lkpm" "${pkgdir}/usr/bin/lkpm"
    if [ -f "./etc/lkpm.d/config.lua" ]; then
        install -Dm644 "./etc/lkpm.d/config.lua" "${pkgdir}/etc/lkpm.d/config.lua"
    fi
    if [ -f "./etc/lkpm.d/mirrorlist" ]; then
        install -Dm644 "./etc/lkpm.d/mirrorlist" "${pkgdir}/etc/lkpm.d/mirrorlist"
    fi
    echo "--> [PACKAGE] Installing lkmake...."
    install -Dm755 "./target/release/lkmake" "${pkgdir}/usr/bin/lkmake"
    echo "--> [PACKAGE] Installing lkinit...."
    install -Dm755 "./target/release/lkinit" "${pkgdir}/usr/bin/lkinit"
    if [ -f "./etc/lkinit.d/init.rs" ]; then
        install -Dm644 "./etc/lkinit.d/init.rs" "${pkgdir}/etc/lkinit.d/init.rs"
    fi
    if [ -f "./etc/lkinit.d/Cargo.toml" ]; then
        install -Dm644 "./etc/lkinit.d/Cargo.toml" "${pkgdir}/etc/lkinit.d/Cargo.toml"
    fi
    echo "--> [PACKAGE] Installing lkchroot...."
    install -Dm755 "./target/release/lkchroot" "${pkgdir}/usr/bin/lkchroot"
}
