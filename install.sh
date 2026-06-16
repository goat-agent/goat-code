set -eu

repo=goat-agent/goat-code
base=https://github.com/$repo/releases/latest/download
install_dir=/usr/local/bin
purple=''
reset=''
if [ -t 1 ]; then
    purple='\033[35m'
    reset='\033[0m'
fi
printf "%bgoat-code%b installer\n" "$purple" "$reset"

need() {
    if ! command -v "$1" >/dev/null 2>&1; then
        echo "missing required command: $1" >&2
        exit 1
    fi
}

need uname
need tar
need mktemp
need grep
need awk
need id

if command -v curl >/dev/null 2>&1; then
    fetch() {
        curl -fsSL "$1" -o "$2"
    }
elif command -v wget >/dev/null 2>&1; then
    fetch() {
        wget -qO "$2" "$1"
    }
else
    echo "missing required command: curl or wget" >&2
    exit 1
fi

os=$(uname -s)
arch=$(uname -m)
case "$os:$arch" in
    Linux:x86_64)
        target=x86_64-unknown-linux-gnu
        ;;
    Linux:aarch64|Linux:arm64)
        target=aarch64-unknown-linux-gnu
        ;;
    Darwin:x86_64)
        target=x86_64-apple-darwin
        ;;
    Darwin:arm64)
        target=aarch64-apple-darwin
        ;;
    *)
        echo "unsupported platform: $os $arch" >&2
        exit 1
        ;;
esac

if [ "$(id -u)" = 0 ]; then
    install_cmd='install'
else
    need sudo
    install_cmd='sudo install'
fi

if command -v sha256sum >/dev/null 2>&1; then
    verify() {
        expected=$(grep "  $archive" "$tmp/SHA256SUMS" | awk '{print $1}')
        actual=$(sha256sum "$tmp/$archive" | awk '{print $1}')
        test -n "$expected" && test "$expected" = "$actual"
    }
elif command -v shasum >/dev/null 2>&1; then
    verify() {
        expected=$(grep "  $archive" "$tmp/SHA256SUMS" | awk '{print $1}')
        actual=$(shasum -a 256 "$tmp/$archive" | awk '{print $1}')
        test -n "$expected" && test "$expected" = "$actual"
    }
else
    echo "missing required command: sha256sum or shasum" >&2
    exit 1
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
archive=goat-code-$target.tar.gz
fetch "$base/$archive" "$tmp/$archive"
fetch "$base/SHA256SUMS" "$tmp/SHA256SUMS"
if ! verify; then
    echo "checksum verification failed for $archive" >&2
    exit 1
fi
mkdir -p "$tmp/extract"
tar -xzf "$tmp/$archive" -C "$tmp/extract"
test -f "$tmp/extract/goat"
test -f "$tmp/extract/goat-update"
$install_cmd -m 755 "$tmp/extract/goat" "$install_dir/goat"
$install_cmd -m 755 "$tmp/extract/goat-update" "$install_dir/goat-update"
printf "%bgoat-code%b installed to %s\n" "$purple" "$reset" "$install_dir/goat"
