# PKGBUILD For lkpm, lkinit, lkchroot, and lkmake

# Contributor: Janorovic Volkov <janorovicvolkov@gmail.com>
# Maintainer: Janorovic Volkov <janorovicvolkov@gmail.com>

pkgname=lkpm
pkgver=1.0.0
pkgrel=1
pkgdesc="Liska Core Toolchain Ecosystem"
arch=('x86_64')
license=('GPL-3.0-or-later')
depends=(
    'ca-certificates' 'sqlite' 'zstd' 'xz' 'busybox' 'cpio' 'kmod' 'bash'
    'curl' 'tar' 'coreutils' 'bsdtar' 'fakeroot' 'gzip' 'bzip2' 'unzip'
    'p7zip'
)
optdepends=(
    'git' 'make' 'gcc' 'clang' 'python' 'perl' 'ruby' 'nodejs'
    'npm' 'yarn' 'go' 'rust'
)
makedepends=('rust' 'pkgconf' 'sqlite')

build() {
    echo "--> [BUILD] Compiling lkpm...."
    cd "${srcdir}/lkpm"
    cargo build --release
    echo "--> [BUILD] Compiling lkmake...."
    cd "${srcdir}/lkmake"
    cargo build --release
    echo "--> [BUILD] Compiling lkinit...."
    cd "${srcdir}/lkinit"
    cargo build --release
    echo "--> [BUILD] Compiling lkchroot...."
    cd "${srcdir}/lkchroot"
    cargo build --release
}

package() {
    install -d "${pkgdir}/usr/bin"
    install -d "${pkgdir}/etc"
    echo "--> [PACKAGE] Installing lkpm...."
    install -Dm755 "${srcdir}/lkpm/target/release/lkpm" "${pkgdir}/usr/bin/lkpm"
    if [ -f "${srcdir}/lkpm/etc/lkpm.d/config.lua" ]; then
        install -Dm644 "${srcdir}/lkpm/etc/lkpm.d/config.lua" "${pkgdir}/etc/lkpm.d/config.lua"
    fi
    if [ -f "${srcdir}/lkpm/etc/lkpm.d/mirrorlist" ]; then
        install -Dm644 "${srcdir}/lkpm/etc/lkpm.d/mirrorlist" "${pkgdir}/etc/lkpm.d/mirrorlist"
    fi
    echo "--> [PACKAGE] Installing lkmake...."
    install -Dm755 "${srcdir}/lkmake/target/release/lkmake" "${pkgdir}/usr/bin/lkmake"
    echo "--> [PACKAGE] Installing lkinit...."
    install -Dm755 "${srcdir}/lkinit/target/release/lkinit" "${pkgdir}/usr/bin/lkinit"
    if [ -f "${srcdir}/lkinit/etc/lkinit.d/init.rs" ]; then
        install -Dm644 "${srcdir}/lkinit/etc/lkinit.d/init.rs" "${pkgdir}/etc/lkinit.d/init.rs"
    fi
    echo "--> [PACKAGE] Installing lkchroot...."
    install -Dm755 "${srcdir}/lkchroot/target/release/lkchroot" "${pkgdir}/usr/bin/lkchroot"
}
