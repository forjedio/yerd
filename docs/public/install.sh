#!/bin/sh
# Yerd headless installer: the CLI, daemon and helper, without the desktop app.
#
#   curl -fsSL https://yerd.app/install.sh | sh
#
# Environment:
#   YERD_VERSION=<tag>       install this release (e.g. v2.2.0) instead of the
#                            latest stable one
#   YERD_INSTALL_DIR=<dir>   install here instead of ~/.yerd/bin (no spaces,
#                            %, $, quotes or backslashes)
#   YERD_NO_MODIFY_PATH=1    don't run `yerd path install`
#
# The download is checked against the release's SHA256SUMS. Docs:
# https://yerd.app/guide/headless

set -eu

CDN="https://files.yerd.app"
RECEIPT="yerd-install-receipt"
MIN_GLIBC_MINOR=34

say() {
    printf '%s\n' "$*"
}

die() {
    printf 'error: %s\n' "$*" >&2
    exit 1
}

need() {
    command -v "$1" >/dev/null 2>&1 || die "this installer needs '$1'; install it and re-run"
}

fetch() {
    curl -fsSL --proto '=https' --tlsv1.2 --retry 3 -o "$2" "$1"
}

detect_platform() {
    os_name=$(uname -s)
    machine=$(uname -m)
    case "$os_name" in
        Linux)
            OS="Linux"
            case "$machine" in
                x86_64 | amd64) ARCH="x86_64" ;;
                aarch64 | arm64) ARCH="Arm64" ;;
                *) die "unsupported architecture: Linux $machine (Yerd supports x86_64 and arm64)" ;;
            esac
            ;;
        Darwin)
            OS="MacOS"
            if [ "$machine" = "x86_64" ] && [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || true)" = "1" ]; then
                machine="arm64"
            fi
            case "$machine" in
                arm64) ARCH="AppleSilicon" ;;
                *) die "unsupported architecture: macOS $machine (Yerd supports Apple Silicon Macs only)" ;;
            esac
            ;;
        *) die "unsupported OS: $os_name (Yerd supports Linux and macOS)" ;;
    esac
}

# The Linux build links against glibc 2.34+. musl distros (e.g. Alpine) and
# older glibc can't load it, so stop before downloading anything.
check_glibc() {
    [ "$OS" = "Linux" ] || return 0
    glibc=$(getconf GNU_LIBC_VERSION 2>/dev/null | awk '{print $2}')
    [ -n "$glibc" ] \
        || die "Yerd's Linux build needs glibc 2.$MIN_GLIBC_MINOR or newer, and this system doesn't use glibc (musl distros such as Alpine aren't supported)"
    major=${glibc%%.*}
    minor=${glibc#*.}
    minor=${minor%%.*}
    case "$major$minor" in
        *[!0-9]*) die "could not read the glibc version ('$glibc'); Yerd needs glibc 2.$MIN_GLIBC_MINOR or newer" ;;
    esac
    if [ "$major" -lt 2 ] || { [ "$major" -eq 2 ] && [ "$minor" -lt "$MIN_GLIBC_MINOR" ]; }; then
        die "this system has glibc $glibc; Yerd's Linux build needs glibc 2.$MIN_GLIBC_MINOR or newer (e.g. Ubuntu 22.04, Debian 12, RHEL 9 or later)"
    fi
}

refuse_desktop_app() {
    if [ "$OS" = "MacOS" ]; then
        set -- "/Applications/Yerd.app" "${HOME:-/nonexistent}/Applications/Yerd.app"
    else
        set -- "/usr/bin/yerdd"
    fi
    for gui in "$@"; do
        if [ -e "$gui" ]; then
            die "the Yerd desktop app is installed ($gui). A headless install can't run alongside it; remove it with \`yerd uninstall\` first, then re-run this script"
        fi
    done
}

# Print the browser_download_url values inside the "stable" entry of
# latest.json, one per line. The manifest is serde output with "stable"
# written before "rc", so the stable entry is the text between those keys,
# whether the JSON is pretty-printed or compact.
stable_urls() {
    flat=$(tr -d '\r\n' <"$1")
    stable=$(printf '%s\n' "$flat" | sed -n 's/.*"stable"[[:space:]]*:\(.*\)"rc"[[:space:]]*:.*/\1/p')
    if [ -z "$stable" ]; then
        stable=$(printf '%s\n' "$flat" | sed -n 's/.*"stable"[[:space:]]*:\(.*\)$/\1/p')
    fi
    printf '%s\n' "$stable" | awk '{
        n = split($0, parts, /"browser_download_url"[ \t]*:[ \t]*"/)
        for (i = 2; i <= n; i++) {
            u = parts[i]
            sub(/".*/, "", u)
            print u
        }
    }'
}

resolve_url() {
    if [ -n "${YERD_VERSION:-}" ]; then
        case "$YERD_VERSION" in
            *[!0-9A-Za-z.-]*) die "YERD_VERSION must be a release tag such as v2.2.0, got '$YERD_VERSION'" ;;
        esac
        TAG="v${YERD_VERSION#v}"
        dashed="v$(printf '%s' "${TAG#v}" | tr '.' '-')"
        URL="$CDN/releases/$TAG/Yerd_${OS}_${ARCH}_${dashed}.headless.tar.gz"
        SUMS_URL="$CDN/releases/$TAG/SHA256SUMS"
        return
    fi
    fetch "$CDN/latest.json" "$TMP/latest.json" || die "could not download $CDN/latest.json"
    URL=""
    SUMS_URL=""
    stable_urls "$TMP/latest.json" >"$TMP/urls"
    while IFS= read -r u; do
        case "$u" in
            */"Yerd_${OS}_${ARCH}_"*.headless.tar.gz) URL="$u" ;;
            */SHA256SUMS) SUMS_URL="$u" ;;
        esac
    done <"$TMP/urls"
    [ -n "$URL" ] \
        || die "no headless build published for $OS/$ARCH in the latest stable release; set YERD_VERSION=<tag> to pick a release"
    [ -n "$SUMS_URL" ] || SUMS_URL="${URL%/*}/SHA256SUMS"
    TAG=$(basename "$(dirname "$URL")")
}

sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    else
        shasum -a 256 "$1" | awk '{print $1}'
    fi
}

verify_checksum() {
    fetch "$SUMS_URL" "$TMP/SHA256SUMS" || die "could not download $SUMS_URL"
    want=$(awk -v n="$NAME" '$2 == n || $2 == "*" n { print $1; exit }' "$TMP/SHA256SUMS")
    [ -n "$want" ] || die "$NAME is not listed in $SUMS_URL"
    got=$(sha256_of "$TMP/$NAME")
    [ "$got" = "$want" ] || die "checksum mismatch for $NAME (expected $want, got $got); the download is corrupt or incomplete; try again"
}

payload_files() {
    if [ "$OS" = "MacOS" ]; then
        say "yerd-helper yerd-forward yerdd yerd $RECEIPT"
    else
        say "yerd-helper yerdd yerd $RECEIPT"
    fi
}

extract_and_check() {
    mkdir "$TMP/x"
    tar -xzf "$TMP/$NAME" -C "$TMP/x" || die "could not extract $NAME"
    for f in $(payload_files); do
        p="$TMP/x/yerd/$f"
        if [ -L "$p" ] || [ ! -f "$p" ]; then
            die "$NAME is missing yerd/$f"
        fi
    done
    receipt=$(tr -d ' \t\r\n' <"$TMP/x/yerd/$RECEIPT")
    [ "$receipt" = "headless" ] || die "$NAME is not a headless build (its install receipt says '$receipt')"
}

install_files() {
    mkdir -p "$DIR" || die "could not create $DIR"
    for f in $(payload_files); do
        mode=0755
        if [ "$f" = "$RECEIPT" ]; then
            mode=0644
        fi
        tmp_target="$DIR/.$f.yerd-install.$$"
        install -m "$mode" "$TMP/x/yerd/$f" "$tmp_target" || die "could not write $tmp_target"
        mv -f "$tmp_target" "$DIR/$f" || die "could not install $DIR/$f"
    done
}

# Re-running the installer over a live install replaces the binaries under a
# running daemon, which keeps running the old version until it restarts.
restart_running_daemon() {
    "$DIR/yerd" ping >/dev/null 2>&1 || return 0
    if "$DIR/yerd" restart daemon >/dev/null 2>&1; then
        say "Restarted the running daemon on Yerd $TAG"
        if [ "$OS" = "Linux" ]; then
            say "If you had granted ports 80/443, run \`sudo $DIR/yerd elevate ports\`,"
            say "then \`$DIR/yerd restart daemon\`: replacing yerdd resets that grant."
        fi
    else
        say "The daemon is still running the previous version; run"
        say "\`$DIR/yerd restart daemon\` to load $TAG."
    fi
}

main() {
    need uname
    detect_platform

    [ "$(id -u)" -ne 0 ] \
        || die "don't run this installer as root: Yerd runs as your user. Run it as your normal account; sudo is only needed for later setup steps"
    check_glibc
    refuse_desktop_app

    need curl
    need tar
    need awk
    need sed
    need mktemp
    need install
    command -v sha256sum >/dev/null 2>&1 || command -v shasum >/dev/null 2>&1 \
        || die "this installer needs 'sha256sum' or 'shasum'; install one and re-run"

    [ -n "${HOME:-}" ] || [ -n "${YERD_INSTALL_DIR:-}" ] || die "HOME is not set; set YERD_INSTALL_DIR"
    DIR="${YERD_INSTALL_DIR:-$HOME/.yerd/bin}"
    case "$DIR" in
        *[[:space:]]* | *[%\$\"\'\\]*) die "the install directory '$DIR' contains whitespace or one of % \$ \" ' \\, which the daemon's service registration can't handle; set YERD_INSTALL_DIR to a plain path" ;;
    esac

    TMP=$(mktemp -d 2>/dev/null || mktemp -d -t yerd-install)
    trap 'rm -rf "$TMP"' EXIT
    trap 'exit 130' INT TERM

    resolve_url
    NAME=${URL##*/}
    say "Downloading Yerd $TAG for $OS/$ARCH"
    say "  $URL"
    fetch "$URL" "$TMP/$NAME" \
        || die "could not download $URL (is $TAG a release with a headless build for $OS/$ARCH?)"
    verify_checksum
    say "Checksum OK"
    extract_and_check
    install_files
    say "Installed Yerd $TAG into $DIR"
    restart_running_daemon

    yerd_cmd="$DIR/yerd"
    if [ "${YERD_NO_MODIFY_PATH:-}" = "1" ]; then
        say ""
        say "Skipped PATH setup (YERD_NO_MODIFY_PATH=1). Add $DIR to your PATH yourself,"
        say "or run \`$DIR/yerd path install\` later."
    elif "$DIR/yerd" path install; then
        yerd_cmd="yerd"
    else
        say ""
        say "Could not add Yerd to your PATH automatically. Add $DIR to your PATH yourself,"
        say "or run \`$DIR/yerd path install\` later."
    fi

    say ""
    say "Next steps:"
    if [ "$OS" = "MacOS" ]; then
        say "  sudo $DIR/yerd daemon install    # run the daemon now and at boot"
    else
        say "  $yerd_cmd daemon install    # run the daemon now and at boot"
    fi
    say "  $yerd_cmd install php 8.5"
    say "  sudo $DIR/yerd elevate    # trust the CA, .test resolver, ports 80/443"
    say ""
    say "sudo needs the full path because its PATH doesn't include $DIR."
    say "Docs: https://yerd.app/guide/headless"
}

main "$@"
