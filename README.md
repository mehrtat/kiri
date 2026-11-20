<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="brand/logo/kiri-horizontal-dark.svg">
    <img src="brand/logo/kiri-horizontal-light.svg" alt="Kiri" width="220">
  </picture>
</p>

<p align="center">
  <b>A tiny, fast container runtime for macOS, Windows and Linux.</b><br>
  Runs your Docker images and works with the <code>docker</code> CLI, Compose and buildx you already use.
</p>

<p align="center">
  <a href="https://runkiri.com/releases/">Download</a> ·
  <a href="docs/features.md">Features</a> ·
  <a href="docs/development.md">Build from source</a> ·
  <a href="https://github.com/libracoder/kiri/releases">Releases</a> ·
  <a href="#about-this-fork">About this fork</a>
</p>

<p align="center">
  <a href="https://github.com/libracoder/kiri/releases/latest"><img alt="Latest release" src="https://img.shields.io/github/v/release/libracoder/kiri?label=release&color=2f6feb"></a>
  <a href="LICENSE"><img alt="License: Apache-2.0" src="https://img.shields.io/badge/license-Apache--2.0-555"></a>
  <a href="#in-your-language"><img alt="38 languages" src="https://img.shields.io/badge/languages-38-2ea043"></a>
</p>

<p align="center">
  🇬🇧 🇮🇩 🇩🇪 🇪🇸 🇫🇷 🇮🇹 🇵🇱 🇧🇷 🇻🇳 🇹🇷 🇷🇺 🇸🇦 🇯🇵 🇰🇷 🇨🇳 🇹🇼<br>
  <sub>Kiri speaks 38 languages. <a href="#in-your-language">Is yours one?</a></sub>
</p>

<p align="center">
  <img src="docs/images/kiri-window.png" alt="The Kiri window, showing containers grouped by Compose project with their state, ports, local domains and size" width="900">
</p>

---

Kiri is a lightweight replacement for Docker Desktop and OrbStack, written in
Rust. It runs containers in one small Linux VM (or straight on the kernel on
Linux) and speaks the Docker Engine API, so your tools just work.

The name comes from the Igbo word *ntakịrị*, "small".

## About this fork

This is a fork of [libracoder/kiri](https://github.com/libracoder/kiri). It
follows upstream and adds fixes that came up running Kiri all day on build
machines, many of them in Iran, where Docker Hub and Docker's download site
answer 403. None of it is specific to one project: it helps anyone whose
networks come and go, whose Linux host ran Docker Engine before, or who
cannot reach Docker Hub.

Get it from [the fork's releases](https://github.com/mehrtat/kiri/releases):
`Kiri-<version>-x86_64.msi` for Windows, `.deb` and `.rpm` for Linux, each
with a `SHA256SUMS` file. The fork numbers its own releases (0.3.0 on), so
its version numbers do not match upstream's.

### What it adds

Pulls and builds where Docker Hub can't be reached:

- **Docker Hub through the fastest mirror.** A pull of a Hub image times Hub
  and its mirrors, starts with the fastest and moves on when one fails,
  keeping the layers it already has. The built-in list has Iranian mirrors
  (ArvanCloud, Liara, Focker, HamDocker, IranServer, MobinHost, Kernel,
  docker.ir) and foreign ones (mirror.gcr.io, Timeweb, DaoCloud). Your own
  mirrors and Docker's `registry-mirrors` come first. Credentials go only to
  Docker Hub, never to a mirror. Kiri's BuildKit builder (`kiri-build`) gets
  the same mirrors for `FROM`.
- **Registries signed by a private CA.** Kiri trusts the system's certificate
  store and, per registry, `certs.d/<host>/` as Docker does.
- **Linux packages build where download.docker.com refuses the country**: the
  docker CLI comes from a mirror, checked against the same pinned SHA-256.

New commands:

```sh
kiri registry mirror            # the order a pull tries, with each host's time
kiri registry mirror test       # also compares the digest each gives for alpine:latest
kiri registry mirror add HOST   # also: rm HOST, builtin on|off
kiri registry trust HOST        # install a registry's CA (and a client certificate)
kiri registry ls                # also: rm HOST
kiri setup                      # Linux: the window's first-run checklist in a terminal
kiri setup all                  # or: running, access, docker, builds
```

Fixes:

- **Containers have a network on Linux hosts that ran Docker Engine, or use
  ufw or firewalld.** Docker leaves the iptables `FORWARD` policy at `DROP`.
  Kiri now adds an accept for its own bridges to `DOCKER-USER` (or `FORWARD`)
  and removes it when it stops.
- **No more `dns: dns bind: Address in use (os error 98)`** after networks
  come and go, as with `docker compose up` and `down` all day. A network's
  DNS server left behind by a missed removal is dropped. An exec's process (a
  `docker exec`, or a healthcheck) no longer keeps a copy of the engine's
  sockets.
- **Windows: Kiri keeps serving `\\.\pipe\kiri` when it is stopped and
  started again at once.** The new kiri-win used to find the pipe still held
  by the old one and quit, leaving no Kiri at all. It now waits for the old
  one to go.
- **Windows installer** built on GitHub with every release.

Each release's notes are in [docs/releases](docs/releases).

### Good to know

- The Windows installer is not Authenticode-signed, so SmartScreen asks
  before running it.
- On Windows, Kiri gives back memory only when WSL lets it. WSL keeps file
  cache to itself unless `%USERPROFILE%\.wslconfig` has:
  ```ini
  [experimental]
  autoMemoryReclaim=dropCache
  ```
  It applies after `wsl --shutdown`, and it covers every WSL distro.
- `docker build` uses whichever buildx builder is current. Another BuildKit
  container (`buildx_buildkit_default`, say) does not get Kiri's mirrors; use
  `--builder kiri-build`, or `docker buildx use kiri-build`.

## Why Kiri

- **Fast**: a container starts and exits in about 2.5 ms through the Docker API. That's around 80 times faster than Docker Desktop.
- **Light**: the VM sleeps after 60 s idle, the daemon stays at about 15 MB, and it wakes in about 0.15 s.
- **Compatible**: `docker`, Compose, buildx, `docker login` and amd64 images through Rosetta.
- **Local domains**: `http://<name>.kiri.local`, with HTTPS from Kiri's own certificate authority.
- **More than containers**: a one-command Kubernetes cluster and Linux machines.
- **A native window** for containers, images, volumes and networks, plus a menu bar item.
- **In your language**: 38 languages, on macOS, Windows and Linux alike.

### In your language

Kiri's window and setup come in these languages. It follows your system's language, and you can change it in Settings → Appearance. In Arabic, Persian and Central Kurdish the whole window reads right to left.

| | Language | | Language | | Language | | Language |
|---|---|---|---|---|---|---|---|
| 🇬🇧 | English | 🇳🇴 | Norsk bokmål | 🇧🇬 | Български | 🇮🇳 | తెలుగు |
| 🇮🇩 | Bahasa Indonesia | 🇵🇱 | Polski | 🇷🇺 | Русский | 🇹🇭 | ไทย |
| 🇦🇩 | Català | 🇧🇷 | Português (Brasil) | 🇺🇦 | Українська | 🇱🇦 | ລາວ |
| 🇨🇿 | Čeština | 🇷🇴 | Română | 🇰🇿 | Қазақша | 🇯🇵 | 日本語 |
| 🇩🇪 | Deutsch | 🇦🇱 | Shqip | 🇸🇦 | العربية | 🇰🇷 | 한국어 |
| 🇪🇸 | Español | 🇸🇮 | Slovenščina | 🇮🇷 | فارسی | 🇨🇳 | 简体中文 |
| 🇫🇷 | Français | 🇫🇮 | Suomi | 🇮🇶 | کوردیی ناوەندی | 🇹🇼 | 繁體中文 |
| 🇮🇹 | Italiano | 🇻🇳 | Tiếng Việt | 🇮🇳 | हिन्दी | 🇪🇹 | አማርኛ |
| 🇱🇹 | Lietuvių | 🇹🇷 | Türkçe | 🇧🇩 | বাংলা |  |  |
| 🇭🇺 | Magyar | 🇬🇷 | Ελληνικά | 🇮🇳 | தமிழ் |  |  |

Missing yours? Translations live in [`crates/kiri/src/ui/lang`](crates/kiri/src/ui/lang); see [Contributing](#contributing).

## Install

| Platform | Get it |
|----------|--------|
| macOS 14+ (Apple silicon or Intel) | `.dmg` from [runkiri.com/releases](https://runkiri.com/releases/), or `brew install --cask libracoder/kiri/kiri` |
| Windows 10 and 11 (x86-64 or Arm, through WSL 2) | `.msi` from [runkiri.com/releases](https://runkiri.com/releases/) |
| Linux (Ubuntu, Fedora and others) | `.deb` or `.rpm` from [runkiri.com/releases](https://runkiri.com/releases/) |

The first time you open Kiri, it sets up a `kiri` Docker context. Kiri updates itself, and checks each new release's signature first.

## Quick start

```sh
docker --context kiri run --rm alpine echo hello
docker --context kiri compose up -d
kiri k8s start                      # single-node Kubernetes as the `kiri` kubectl context
kiri machine create dev --distro ubuntu && kiri machine shell dev
kiri import                         # bring images, volumes and containers over from OrbStack or Docker Desktop
```

Docker's commands also work as `kiri` commands (`kiri run`, `kiri ps`,
`kiri compose up`, and so on), and always target Kiri.

## Numbers

`alpine` running `true` and removed, median of 30 runs, same `docker` CLI for all three:

|                | Docker Engine API | `docker` CLI |
|----------------|------------------:|-------------:|
| **Kiri**       | **2.55 ms**       | **14.2 ms**  |
| Docker Desktop | 213 ms            | 243 ms       |
| OrbStack       | 380 ms            | 442 ms       |

Method and every other figure are in [`bench/results.json`](bench/results.json). Rerun with `kiri bench --network bridge --runs 30` while all three engines are running.

## How it works

```
docker CLI ─┐
compose ────┼─ Docker Engine API ─► kiri daemon ── vsock ──► guest agent (PID 1)
buildx ─────┤                         owns the VM           clone3 → container
kiri UI ────┘
```

The VM uses a custom 10 MB kernel, and its PID 1 is a ~900 KB Rust agent.
That agent starts containers with a single `clone3` into namespaces and a
cgroup. There's no runc, no shim and no dockerd. More detail is in
[docs/development.md](docs/development.md#architecture).

## Documentation

- [Features](docs/features.md): everything supported today, release files and known issues
- [Development](docs/development.md): building, running from source, architecture and repository layout
- [Release notes](docs/releases)

## Roadmap

Next are more ways to share (Tailscale and Cloudflare Tunnel, in 0.3.0) and
Coolify support (0.4.0). See [ROADMAP.md](ROADMAP.md) for what's in each
release.

## Contributing

Issues and pull requests are welcome. See [CONTRIBUTING.md](CONTRIBUTING.md)
and the [Code of Conduct](CODE_OF_CONDUCT.md). To report a vulnerability
privately, see [SECURITY.md](SECURITY.md).

## License

[Apache-2.0](LICENSE). Third-party licences are in [NOTICE](NOTICE). The guest
kernel is Linux (GPL-2.0); its config, patches and the source offer are in
[`guest/kernel`](guest/kernel).

The name "Kiri" and the Kiri logo are not covered by the licence. Forks are
welcome, under a different name and logo.
