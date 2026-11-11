# Shared Runtime Manager

A shared dependency manager for locally installed JulianoDev desktop applications.

The goal is to install large tools once and let multiple applications use them safely, while keeping application data and settings independent.

## Current status

**Early development (0.1.0).** The first working slice targets Windows and manages FFmpeg. The registry, application links, download cache, staged installation, and SHA-256 verification are in place. SoX, Python, Node.js, model management, automatic updates, and a graphical interface are not implemented yet.

## Shared location

On Windows, data is stored under:

```text
%LOCALAPPDATA%\JulianoDev\SharedRuntime\
├── cache/
├── models/
├── registry/
│   └── state.json
├── runtimes/
└── tools/
    └── ffmpeg/
        └── gyan-release-essentials/
            ├── ffmpeg.exe
            └── ffprobe.exe
```

The manager does not add global PATH entries and does not modify application folders.

## Build

Install the Rust stable toolchain, then:

```powershell
cargo build --release
```

The executable is `target/release/srm.exe` on Windows.

## First run

Register each application once:

```powershell
srm register ttk-media --name "TTK Media"
srm register nova-generator --name "Nova Generator"
```

Install FFmpeg and link it to an application:

```powershell
srm install ffmpeg --app ttk-media
```

The same installation can be linked to another application:

```powershell
srm install ffmpeg --app nova-generator
```

Resolve a resource path for an application:

```powershell
srm resolve ffmpeg --app ttk-media
```

Other commands:

```powershell
srm root
srm apps
srm resources
```

## Safety model

- Downloads are written to a cache and extracted into a staging directory.
- ZIP entries are checked for enclosed paths; only the two expected FFmpeg executables are extracted.
- The registry records SHA-256 hashes for installed executables.
- A resource is only resolved after its files pass integrity checks.
- Applications are linked explicitly in the registry.
- There is intentionally no uninstall command in this first version. Removal and updates must account for every application reference before they are introduced.

## Download source

The FFmpeg Windows essentials archive is obtained from Gyan.dev: https://www.gyan.dev/ffmpeg/builds/

The archive endpoint is a rolling release. The manager records the source and hashes the binaries after installation, but does not yet verify a publisher-provided signature or pinned upstream checksum. Do not treat this first version as a supply-chain hardened installer.

## Roadmap

1. Windows FFmpeg manager and application registry.
2. SoX and additional tools with pinned versions and verified checksums.
3. Versioned Python and Node.js runtimes.
4. Shared model storage with per-application references.
5. Safe update/removal with reference checks and rollback.
6. Tauri integration and a small desktop management screen.
