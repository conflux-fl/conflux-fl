#!/bin/sh
# Installs `cflux`, the Conflux FL command line.
#
#   curl -fsSL https://confluxfl.dev/install.sh | sh
#
# What it does, in order: works out which release asset suits this
# machine, downloads it with its published SHA-256, **verifies the
# checksum before unpacking anything**, and puts one binary on disk. It
# needs no toolchain, no root, and writes nothing outside the install
# directory.
#
# Piping a script into a shell is a real trust decision, so this one
# tries to earn it: it is POSIX sh, it is short enough to read
# (`curl -fsSL https://confluxfl.dev/install.sh | less`), it verifies
# what it downloads, and it refuses rather than guesses when this machine
# cannot run the binary.
#
# Options:
#   --version vX.Y.Z   install a specific release (default: latest)
#   --dir PATH         where to install (default: ~/.local/bin)
#   --help
#
# The canonical copy lives in the conflux-fl repository; the copy served
# from confluxfl.dev is deployed from it. Edit it there.

set -eu

REPO="conflux-fl/conflux-fl"
VERSION=""
INSTALL_DIR="${HOME}/.local/bin"
# glibc floor of the published Linux binary, set by the runner it is
# built on. Checked rather than discovered at first run, because
# "version `GLIBC_2.34' not found" is not a message anyone should have to
# interpret.
MIN_GLIBC="2.34"

# ---------------------------------------------------------------------
# Output. Colour only when stdout is a terminal — a piped or logged run
# should not be full of escape codes.
# ---------------------------------------------------------------------
if [ -t 1 ]; then
	BOLD=$(printf '\033[1m')
	RED=$(printf '\033[31m')
	DIM=$(printf '\033[2m')
	OFF=$(printf '\033[0m')
else
	BOLD=""
	RED=""
	DIM=""
	OFF=""
fi

say() { printf '%s\n' "$*"; }
step() { printf '%s==>%s %s\n' "$BOLD" "$OFF" "$*"; }
# Failures go to stderr and exit non-zero, so a wrapper script can tell
# a failed install from a slow one.
die() {
	printf '%serror:%s %s\n' "$RED" "$OFF" "$*" >&2
	exit 1
}

# Written out rather than read back from "$0": the intended way to run
# this is piped from curl, where there is no file to read and `--help`
# would silently print nothing.
usage() {
	cat <<-EOF
		Installs cflux, the Conflux FL command line.

		  curl -fsSL https://confluxfl.dev/install.sh | sh

		Downloads the release asset for this machine, verifies its
		published SHA-256 before unpacking, and installs one binary.
		No toolchain, no root.

		Options:
		  --version vX.Y.Z   install a specific release (default: latest)
		  --dir PATH         where to install (default: \$HOME/.local/bin)
		  --help

		To pass options through a pipe, separate them with --:
		  curl -fsSL https://confluxfl.dev/install.sh | sh -s -- --dir ~/bin

		Full documentation: https://confluxfl.dev/installation/
	EOF
	exit 0
}

while [ $# -gt 0 ]; do
	case "$1" in
	--version)
		[ $# -ge 2 ] || die "--version needs a value, e.g. --version vX.Y.Z"
		VERSION="$2"
		shift 2
		;;
	--dir)
		[ $# -ge 2 ] || die "--dir needs a path"
		INSTALL_DIR="$2"
		shift 2
		;;
	--help | -h) usage ;;
	*) die "unknown option: $1 (try --help)" ;;
	esac
done

# ---------------------------------------------------------------------
# Which download suits this machine
# ---------------------------------------------------------------------
detect_target() {
	os=$(uname -s)
	arch=$(uname -m)

	case "$os" in
	Linux)
		# The published Linux binary links glibc. On musl (Alpine) it
		# will not start, and the failure is an unhelpful "not found"
		# from the loader, so it is refused here instead.
		if [ -f /etc/alpine-release ] || (ldd --version 2>&1 | grep -qi musl); then
			die "this build links glibc and will not run on musl (Alpine).
  Build from source instead:
    cargo install --git https://github.com/${REPO} cflux"
		fi
		case "$arch" in
		x86_64 | amd64) TARGET="x86_64-unknown-linux-gnu" ;;
		*) die "no prebuilt Linux binary for ${arch} yet.
  Build from source instead:
    cargo install --git https://github.com/${REPO} cflux" ;;
		esac
		ARCHIVE_EXT="tar.gz"
		;;
	Darwin)
		case "$arch" in
		arm64) TARGET="aarch64-apple-darwin" ;;
		x86_64) TARGET="x86_64-apple-darwin" ;;
		*) die "unrecognised macOS architecture: ${arch}" ;;
		esac
		ARCHIVE_EXT="tar.gz"
		;;
	MINGW* | MSYS* | CYGWIN*)
		die "Windows is not installed by this script.
  Download the .zip and verify it as described at
  https://confluxfl.dev/installation/"
		;;
	*)
		die "unsupported operating system: ${os}"
		;;
	esac
}

# The binary needs a glibc at least as new as the one it was built
# against. Told now, with the distributions that qualify, rather than
# discovered as a loader error later.
check_glibc() {
	[ "$(uname -s)" = "Linux" ] || return 0
	command -v ldd >/dev/null 2>&1 || return 0
	have=$(ldd --version 2>/dev/null | head -1 | grep -oE '[0-9]+\.[0-9]+$' || true)
	[ -n "$have" ] || return 0
	# `sort -V` puts the lower version first; if that is not the floor,
	# this system is older than the floor.
	oldest=$(printf '%s\n%s\n' "$MIN_GLIBC" "$have" | sort -V | head -1)
	if [ "$oldest" != "$MIN_GLIBC" ]; then
		die "this build needs glibc ${MIN_GLIBC} or newer; this system has ${have}.
  Ubuntu 22.04+, Debian 12+, RHEL/Rocky 9+ and Amazon Linux 2023 qualify.
  On an older system, build from source instead:
    cargo install --git https://github.com/${REPO} cflux"
	fi
}

# ---------------------------------------------------------------------
# Which release
# ---------------------------------------------------------------------
# Resolved by following the /releases/latest redirect rather than by
# calling the API: the redirect is not rate-limited and needs no token,
# and an unauthenticated API call from a shared network often is.
resolve_version() {
	[ -z "$VERSION" ] || return 0
	url=$(curl -fsSLI -o /dev/null -w '%{url_effective}' \
		"https://github.com/${REPO}/releases/latest" 2>/dev/null || true)
	VERSION=${url##*/tag/}
	case "$VERSION" in
	v*) : ;;
	# Generic rather than naming a release: a concrete version here
	# would be stale the day after the next one ships, in the one
	# message someone reads when things are already going wrong.
	*) die "could not work out the latest version.
  Pass one explicitly:  --version vX.Y.Z
  Releases: https://github.com/${REPO}/releases" ;;
	esac
}

# ---------------------------------------------------------------------
# Download, verify, install
# ---------------------------------------------------------------------
main() {
	command -v curl >/dev/null 2>&1 || die "curl is required"
	command -v tar >/dev/null 2>&1 || die "tar is required"

	detect_target
	check_glibc
	resolve_version

	name="cflux-${VERSION}-${TARGET}"
	archive="${name}.${ARCHIVE_EXT}"
	base="https://github.com/${REPO}/releases/download/${VERSION}"

	tmp=$(mktemp -d 2>/dev/null || mktemp -d -t cflux)
	# Removed however this exits, including the checksum failure below —
	# a half-downloaded archive should not be left behind for someone to
	# find later and trust.
	trap 'rm -rf "$tmp"' EXIT INT TERM

	step "Installing cflux ${VERSION} for ${TARGET}"

	say "  downloading ${archive}"
	# -f -s -L, deliberately without -S: curl's own error text duplicates
	# the message below and says less, so only one of them should reach
	# the user.
	curl -fsL "${base}/${archive}" -o "${tmp}/${archive}" ||
		die "could not download ${base}/${archive}
  Check that ${VERSION} exists and has an asset for ${TARGET}.
  Releases: https://github.com/${REPO}/releases"
	curl -fsL "${base}/${archive}.sha256" -o "${tmp}/${archive}.sha256" ||
		die "could not download the checksum for ${archive}"

	say "  verifying checksum"
	# Before unpacking, never after: the point of the checksum is to
	# decide whether to trust the bytes, and unpacking is already
	# trusting them.
	expected=$(cut -d' ' -f1 <"${tmp}/${archive}.sha256")
	if command -v sha256sum >/dev/null 2>&1; then
		actual=$(sha256sum "${tmp}/${archive}" | cut -d' ' -f1)
	elif command -v shasum >/dev/null 2>&1; then
		actual=$(shasum -a 256 "${tmp}/${archive}" | cut -d' ' -f1)
	else
		die "need sha256sum or shasum to verify the download"
	fi
	[ "$expected" = "$actual" ] || die "checksum mismatch — refusing to install.
  expected ${expected}
  got      ${actual}
  Download it again; if it fails twice, do not run the binary."

	say "  unpacking"
	tar xzf "${tmp}/${archive}" -C "$tmp" || die "could not unpack ${archive}"
	[ -f "${tmp}/${name}/cflux" ] || die "the archive did not contain cflux"

	mkdir -p "$INSTALL_DIR" 2>/dev/null || die "could not create ${INSTALL_DIR}"
	install -m 755 "${tmp}/${name}/cflux" "${INSTALL_DIR}/cflux" 2>/dev/null ||
		{
			cp "${tmp}/${name}/cflux" "${INSTALL_DIR}/cflux" &&
				chmod 755 "${INSTALL_DIR}/cflux"
		} ||
		die "could not write to ${INSTALL_DIR}
  Pick somewhere writable:  --dir \"\$HOME/bin\""

	# macOS quarantines anything downloaded from the internet, and
	# refuses to run an unsigned quarantined binary. Cleared only here,
	# after the checksum passed — verifying the file is what makes
	# clearing the flag reasonable rather than reckless.
	if [ "$(uname -s)" = "Darwin" ] && command -v xattr >/dev/null 2>&1; then
		xattr -d com.apple.quarantine "${INSTALL_DIR}/cflux" 2>/dev/null || true
	fi

	# Run it. An install that reports success without the binary having
	# executed once has not established very much.
	if ! reported=$("${INSTALL_DIR}/cflux" version 2>/dev/null); then
		die "installed to ${INSTALL_DIR}/cflux but it would not run.
  See https://confluxfl.dev/installation/#when-something-goes-wrong"
	fi

	say ""
	step "Installed: ${reported}"
	say "  ${DIM}${INSTALL_DIR}/cflux${OFF}"

	case ":${PATH}:" in
	*":${INSTALL_DIR}:"*)
		say ""
		say "Try:  cflux catalog list"
		;;
	*)
		say ""
		say "${INSTALL_DIR} is not on your PATH. Add it:"
		say ""
		# Expanded rather than left as `~`: this line is printed for
		# someone to paste, and an absolute path is unambiguous about
		# which file it means.
		case "${SHELL:-}" in
		*zsh) rc="${HOME}/.zshrc" ;;
		*) rc="${HOME}/.bashrc" ;;
		esac
		say "  echo 'export PATH=\"${INSTALL_DIR}:\$PATH\"' >> ${rc}"
		say ""
		say "Or run it directly:  ${INSTALL_DIR}/cflux catalog list"
		;;
	esac
}

main
