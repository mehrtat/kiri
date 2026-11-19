#!/bin/bash
# The docker CLI, Compose and buildx the full `kiri` package bundles, at the
# versions and SHA-256s pinned in crates/kiri/src/cli_pins.rs (LINUX_* for
# x86_64, LINUX_ARM64_* for aarch64):
#   packaging/linux/fetch-cli.sh <cli-dir> [cache-dir] [x86_64|aarch64]
# Leaves <cli-dir>/docker, docker-compose, docker-buildx, each verified.
set -euo pipefail
dir=$1 cache=${2:-${XDG_CACHE_HOME:-$HOME/.cache}/kiri-linux-cli} arch=${3:-x86_64}
# The pins' prefix, and each project's name for the architecture.
case $arch in
  x86_64) p=LINUX_ compose_arch=x86_64 go_arch=amd64 ;;
  aarch64) p=LINUX_ARM64_ compose_arch=aarch64 go_arch=arm64 ;;
  *) echo "arch must be x86_64 or aarch64" >&2; exit 1 ;;
esac
root=$(cd "$(dirname "$0")/../.." && pwd)
pins=$root/crates/kiri/src/cli_pins.rs
pin() { sed -n "s/^pub const $1: (&str, &str) = (\"\([^\"]*\)\", \"\([0-9a-f]*\)\");.*/\1 \2/p" "$pins" | tr -d '\r'; }
read -r dv dsum <<<"$(pin ${p}DOCKER_CLI)"
read -r cv csum <<<"$(pin ${p}COMPOSE)"
read -r bv bsum <<<"$(pin ${p}BUILDX)"
[ -n "$dv" ] && [ -n "$cv" ] && [ -n "$bv" ] || { echo "no ${p}* pins in $pins" >&2; exit 1; }
mkdir -p "$dir" "$cache"
dir=$(cd "$dir" && pwd)
get() { # file sum url... (each tried in turn: a mirror that fails or sends other bytes is skipped)
  local name=$1 f=$cache/$1 sum=$2 url
  shift 2
  if [ -f "$f" ] && echo "$sum  $f" | sha256sum -c --quiet - 2>/dev/null; then
    return
  fi
  rm -f "$f"
  for url in "$@"; do
    # HTTPS only, redirects included; a failed download leaves no .part behind.
    # The .part is this process's own, as CI jobs on one machine share the cache.
    if curl -fsSL --proto '=https' --proto-redir '=https' --retry 3 -o "$f.$$.part" "$url" \
      && echo "$sum  $f.$$.part" | sha256sum -c --quiet - 2>/dev/null; then
      mv "$f.$$.part" "$f"
      return
    fi
    rm -f "$f.$$.part"
    echo "could not download $name from $url (or it does not match the pinned SHA-256); trying the next" >&2
  done
  echo "could not download $name from any source" >&2
  exit 1
}
# download.docker.com refuses some countries (403 from Iran); these mirror
# it file for file, and the pinned SHA-256 decides what is kept.
static=linux/static/stable/$arch/docker-$dv.tgz
get "docker-$dv-$arch.tgz" "$dsum" "https://download.docker.com/$static" \
  "https://mirrors.tuna.tsinghua.edu.cn/docker-ce/$static" "https://mirrors.ustc.edu.cn/docker-ce/$static"
get "docker-compose-$cv-$arch" "$csum" "https://github.com/docker/compose/releases/download/$cv/docker-compose-linux-$compose_arch"
get "buildx-$bv-$arch" "$bsum" "https://github.com/docker/buildx/releases/download/$bv/buildx-$bv.linux-$go_arch"
tar xzf "$cache/docker-$dv-$arch.tgz" -C "$dir" --strip-components=1 docker/docker
install -m 755 "$cache/docker-compose-$cv-$arch" "$dir/docker-compose"
install -m 755 "$cache/buildx-$bv-$arch" "$dir/docker-buildx"
echo "$dir: docker $dv, compose $cv, buildx $bv ($arch)"
