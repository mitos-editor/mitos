#!/bin/sh
# Install a verified Mitos release without changing shell configuration.
set -eu

fail() { printf 'Mitos installer: %s\n' "$*" >&2; exit 1; }
usage() {
  cat <<'HELP'
Usage: install.sh [--channel stable|nightly] [--version VERSION] [--install-dir DIR] [--bin-dir DIR]

Install the latest Mitos release for Linux or macOS without sudo.
  --channel CHANNEL  Choose stable (default) or nightly. Nightly needs python3.
  --version VERSION  Install a stable version or a dated nightly tag.
  --install-dir DIR  Version directory root (default: $XDG_DATA_HOME/mitos or ~/.local/share/mitos).
  --bin-dir DIR      Directory for the ms symlink (default: ~/.local/bin).
  --help             Show this help.
HELP
}

# Keep downloads and redirects on HTTPS, following rustup's downloader pattern.
download() {
  curl --proto '=https' --proto-redir '=https' --tlsv1.2 \
    --fail --location --silent --show-error --retry 3 "$@"
}

# Define the installer before running it, so truncated curl | sh input cannot
# start downloading or modifying an installation.
main() {
  version=latest
  channel=stable
  : "${HOME:?HOME must be set}"
  case "${XDG_DATA_HOME:-}" in
    /*) install_dir="$XDG_DATA_HOME/mitos" ;;
    *) install_dir="$HOME/.local/share/mitos" ;;
  esac
  bin_dir="$HOME/.local/bin"
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --channel | --version | --install-dir | --bin-dir)
        [ "$#" -ge 2 ] && [ -n "$2" ] || fail "Missing value for $1"
        case "$1" in
          --channel) channel=$2 ;;
          --version) version=$2 ;;
          --install-dir) install_dir=$2 ;;
          --bin-dir) bin_dir=$2 ;;
        esac
        shift 2 ;;
      --help | -h) usage; exit 0 ;;
      *) fail "Unknown option: $1 (use --help)" ;;
    esac
  done

  case "$channel" in stable | nightly) ;; *) fail "Unknown channel: $channel" ;; esac

  for tool in curl tar; do
    command -v "$tool" >/dev/null 2>&1 || fail "Required command not found: $tool"
  done
  if command -v sha256sum >/dev/null 2>&1; then
    hash_tool=sha256sum
  elif command -v shasum >/dev/null 2>&1; then
    hash_tool=shasum
  else
    fail 'Install sha256sum or shasum to verify downloads.'
  fi
  case "$(uname -s)" in
    Linux) os=linux ;;
    Darwin) os=macos ;;
    *) fail 'Supported systems are Linux and macOS; Windows uses install.ps1.' ;;
  esac
  case "$(uname -m)" in
    x86_64 | amd64) arch=x86_64 ;;
    arm64 | aarch64) arch=aarch64 ;;
    *) fail "Unsupported CPU architecture: $(uname -m)" ;;
  esac

  work_dir=$(mktemp -d)
  link_tmp=
  cleanup() {
    rm -rf "$work_dir"
    if [ -n "$link_tmp" ]; then rm -f "$link_tmp"; fi
  }
  trap cleanup EXIT
  trap 'exit 1' HUP INT TERM

  if [ "$version" = latest ] && [ "$channel" = nightly ]; then
    command -v python3 >/dev/null 2>&1 || fail 'Nightly selection requires python3, or pass --version nightly-DATE-COMMIT.'
    download 'https://api.github.com/repos/mitos-editor/mitos/releases?per_page=100' \
      --output "$work_dir/releases.json"
    version=$(python3 -c '
import json, re, sys
releases = [r for r in json.load(open(sys.argv[1])) if r["prerelease"] and not r["draft"]
            and re.fullmatch(r"nightly-\d{4}-\d{2}-\d{2}-[0-9a-f]{12}", r["tag_name"])]
if not releases:
    sys.exit("No nightly builds are available.")
print(max(releases, key=lambda r: r["published_at"])["tag_name"])
' "$work_dir/releases.json") || fail 'Could not select a nightly build.'
  elif [ "$version" = latest ]; then
    # The stable redirect needs no JSON parser or GitHub CLI.
    release_url=$(download \
      --output /dev/null --write-out '%{url_effective}' \
      https://github.com/mitos-editor/mitos/releases/latest)
    version=${release_url##*/}
  fi
  version=${version#v}
  if printf '%s\n' "$version" | LC_ALL=C grep -Eq '^nightly-[0-9]{4}-[0-9]{2}-[0-9]{2}-[0-9a-f]{12}$'; then
    tag=$version
  else
    printf '%s\n' "$version" | LC_ALL=C grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$' \
      || fail "Expected a stable version or dated nightly tag, got: $version"
    tag="v$version"
  fi
  package="mitos-$tag-$arch-$os"
  archive="$package.tar.xz"
  release_url="https://github.com/mitos-editor/mitos/releases/download/$tag"
  printf 'Downloading Mitos %s for %s/%s...\n' "$version" "$os" "$arch"
  download \
    "$release_url/$archive" --output "$work_dir/$archive"

  # The first release predates SHA256SUMS; its GitHub asset digests are hosted
  # with the installer. Later releases publish SHA256SUMS as a release asset.
  if [ "$tag" = v0.1.0 ]; then
    checksum_url="https://mitos.computer/checksums/$tag.txt"
  else
    checksum_url="$release_url/SHA256SUMS"
  fi
  download \
    "$checksum_url" --output "$work_dir/SHA256SUMS"
  expected=$(awk -v name="$archive" '$2 == name { print $1 }' "$work_dir/SHA256SUMS")
  [ "${#expected}" -eq 64 ] || fail "No unique SHA-256 checksum found for $archive"
  case "$expected" in *[!0-9a-fA-F]*) fail 'Invalid SHA-256 checksum.' ;; esac
  if [ "$hash_tool" = sha256sum ]; then
    actual=$(sha256sum "$work_dir/$archive" | awk '{print $1}')
  else
    actual=$(shasum -a 256 "$work_dir/$archive" | awk '{print $1}')
  fi
  [ "$actual" = "$expected" ] || fail 'Checksum mismatch; nothing was installed.'

  tar -xJf "$work_dir/$archive" -C "$work_dir"
  [ -x "$work_dir/$package/ms" ] && [ -d "$work_dir/$package/runtime" ] \
    || fail 'The archive does not contain an executable and its runtime.'
  "$work_dir/$package/ms" --version >/dev/null \
    || fail 'This binary cannot run on your system. See the building-from-source guide.'

  mkdir -p "$install_dir" "$bin_dir"
  install_dir=$(cd "$install_dir" && pwd -P)
  bin_dir=$(cd "$bin_dir" && pwd -P)
  # Update only a symlink previously created within this installation root.
  if [ -L "$bin_dir/ms" ]; then
    [ ! -d "$bin_dir/ms" ] || fail "$bin_dir/ms points to a directory. Choose --bin-dir."
    case "$(readlink "$bin_dir/ms")" in
      "$install_dir"/*/ms) ;;
      *) fail "$bin_dir/ms belongs to another installation. Choose --bin-dir." ;;
    esac
  elif [ -e "$bin_dir/ms" ]; then
    fail "$bin_dir/ms already exists. Choose --bin-dir or move it first."
  fi
  destination="$install_dir/$tag-$arch-$os"
  if [ -e "$destination" ] || [ -L "$destination" ]; then
    [ ! -L "$destination" ] && [ -x "$destination/ms" ] && [ -d "$destination/runtime" ] \
      || fail "Incomplete installation already exists at $destination"
  else
    mv "$work_dir/$package" "$destination"
  fi
  link_tmp="$bin_dir/.ms-install-$$"
  ln -s "$destination/ms" "$link_tmp"
  mv -f "$link_tmp" "$bin_dir/ms"
  link_tmp=
  printf 'Installed Mitos %s. Run: ms --health\n' "$version"
  # Print shell expressions literally for the user to copy.
  # shellcheck disable=SC2016
  case ":${PATH:-}:" in
    *":$bin_dir:"*) ;;
    *)
      printf '\nAdd this directory to your shell PATH: %s\n' "$bin_dir"
      if [ "$bin_dir" = "$HOME/.local/bin" ]; then
        printf 'Bash/Zsh: export PATH="$HOME/.local/bin:$PATH"\n'
        printf 'Fish:     fish_add_path "$HOME/.local/bin"\n'
      else
        printf 'Bash/Zsh: export PATH="%s:$PATH"\n' "$bin_dir"
        printf 'Fish:     fish_add_path "%s"\n' "$bin_dir"
      fi ;;
  esac
}

# The shell must read this complete block before it can invoke the installer.
{
  main "$@"
}
