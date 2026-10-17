#!/usr/bin/env bash
# Install or update Neutron Mail on Fedora from the latest GitHub release.
#
#   curl -fsSL https://raw.githubusercontent.com/mkd47/neutron-mail/main/scripts/install.sh | bash
#
# or, from a checkout:  scripts/install.sh [VERSION] [--force]
#
# Downloads the RPM and SHA256SUMS of the release, checks the checksum, and
# installs with dnf (which upgrades an older version and keeps your
# accounts and settings). Needs: curl, sha256sum, dnf, sudo.
set -euo pipefail

REPO="mkd47/neutron-mail"
version=""
force=0
for arg in "$@"; do
  case "$arg" in
    --force) force=1 ;;
    -h|--help) sed -n '2,11p' "$0" 2>/dev/null | sed 's/^# \{0,1\}//'; exit 0 ;;
    v*|[0-9]*) version="${arg#v}" ;;
    *) echo "Unknown option: $arg" >&2; exit 2 ;;
  esac
done

say() { printf '\033[1m%s\033[0m\n' "$*"; }
die() { printf 'Error: %s\n' "$*" >&2; exit 1; }

for cmd in curl sha256sum dnf; do
  command -v "$cmd" >/dev/null || die "$cmd is required (this script is for Fedora)."
done
[ "$(uname -m)" = x86_64 ] || die "only x86_64 builds are published."

if [ -z "$version" ]; then
  say "Looking up the latest release…"
  json="$(curl -fsSL -H 'Accept: application/vnd.github+json' "https://api.github.com/repos/$REPO/releases/latest")" \
    || die "could not reach GitHub."
  tag="$(printf '%s' "$json" | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n1)"
  [ -n "$tag" ] || die "could not read the latest version from GitHub."
  version="${tag#v}"
fi
case "$version" in
  *[!0-9.]*|'') die "unexpected version \"$version\"." ;;
esac

installed="$(rpm -q --qf '%{VERSION}' neutron-mail 2>/dev/null || true)"
case "$installed" in *"not installed"*) installed="" ;; esac
if [ "$installed" = "$version" ] && [ "$force" = 0 ]; then
  say "Neutron Mail $version is already installed. (Use --force to reinstall.)"
  exit 0
fi

rpm_name="neutron-mail-$version-1.x86_64.rpm"
base="https://github.com/$REPO/releases/download/v$version"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

say "Downloading Neutron Mail $version…"
curl -fL --progress-bar -o "$tmp/$rpm_name" "$base/$rpm_name" || die "download failed: $base/$rpm_name"
curl -fsSL -o "$tmp/SHA256SUMS" "$base/SHA256SUMS" || die "could not download SHA256SUMS."

say "Checking the download…"
(cd "$tmp" && grep -F " $rpm_name" SHA256SUMS | sha256sum -c -) \
  || die "checksum mismatch: the download is damaged or was changed. Nothing was installed."

say "Installing (you may be asked for your password)…"
if [ "$(id -u)" = 0 ]; then dnf install -y "$tmp/$rpm_name"; else sudo dnf install -y "$tmp/$rpm_name"; fi

if [ -n "$installed" ]; then
  say "Updated Neutron Mail $installed → $version. Restart the app to use the new version."
else
  say "Installed Neutron Mail $version. Find it in your applications."
fi
