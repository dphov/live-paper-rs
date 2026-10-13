# Maintainer: Dmitry Petukhov <mail@dphov.com>
pkgname=live-paper-fork
pkgver=0.4.1.r2
pkgrel=1
pkgdesc="A Wayland video wallpaper engine (fork with EGL teardown fix)"
arch=('x86_64')
url="https://github.com/dphov/live-paper-rs"
license=('MIT')
depends=('mpv' 'wayland' 'libglvnd')
makedepends=('cargo' 'git')
provides=('live-paper')
conflicts=('live-paper')
replaces=('live-paper')
_commit=b897d38
source=("$pkgname-$pkgver.tar.gz::$url/archive/$_commit.tar.gz")
sha256sums=('SKIP')

prepare() {
    cd "$srcdir/live-paper-rs-$_commit"
    export RUSTUP_TOOLCHAIN=stable
    cargo fetch --locked --target "$(rustc -vV | sed -n 's/host: //p')"
}

build() {
    cd "$srcdir/live-paper-rs-$_commit"
    export RUSTUP_TOOLCHAIN=stable
    export CARGO_TARGET_DIR=target
    cargo build --frozen --release --all-features
}

package() {
    cd "$srcdir/live-paper-rs-$_commit"

    install -Dm755 "target/release/live-paper" "$pkgdir/usr/bin/live-paper"
    install -Dm644 "LICENSE" "$pkgdir/usr/share/licenses/$pkgname/LICENSE"

    install -Dm644 "config.example.toml" \
        "$pkgdir/usr/share/doc/$pkgname/config.example.toml"
}
