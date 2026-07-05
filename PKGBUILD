# Maintainer: Heitor Faria <podheitor@users.noreply.github.com>
# Native BigLinux/Arch package — GPU (Vulkan) build from source.
pkgname=bigtranscriber
pkgver=0.1.0
pkgrel=1
pkgdesc="Live, offline transcription of system audio + microphone for online hearings (GPU/Vulkan)"
arch=('x86_64')
url="https://github.com/podheitor/BigTranscriber"
license=('AGPL-3.0-or-later')
depends=('webkit2gtk-4.1' 'gtk3' 'vulkan-icd-loader' 'pipewire')
makedepends=('rust' 'cargo' 'cmake' 'clang' 'shaderc' 'vulkan-headers' 'git')
optdepends=('python-google-api-python-client: auto-email via Gmail API'
            'python-google-auth: auto-email via Gmail API')
source=("git+https://github.com/podheitor/BigTranscriber.git#tag=v$pkgver")
sha256sums=('SKIP')

build() {
  cd "$srcdir/BigTranscriber/src-tauri"
  # Use the system Vulkan headers/loader + glslc (makedepends above).
  export VULKAN_SDK=/usr
  export CPATH=/usr/include
  cargo build --release --locked
}

package() {
  cd "$srcdir/BigTranscriber"
  install -Dm755 src-tauri/target/release/bigtranscriber "$pkgdir/usr/bin/bigtranscriber"
  install -Dm644 packaging/bigtranscriber.desktop "$pkgdir/usr/share/applications/bigtranscriber.desktop"
  install -Dm644 src-tauri/icons/bigtranscriber.svg "$pkgdir/usr/share/icons/hicolor/scalable/apps/bigtranscriber.svg"
  install -Dm644 src-tauri/icons/128x128.png "$pkgdir/usr/share/icons/hicolor/128x128/apps/bigtranscriber.png"
  install -Dm644 LICENSE "$pkgdir/usr/share/licenses/$pkgname/LICENSE"
}
