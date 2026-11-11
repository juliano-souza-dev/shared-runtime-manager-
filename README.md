# Shared Runtime Manager

A shared dependency manager for locally installed JulianoDev desktop applications.

The goal is to install large tools once and let multiple applications use them safely, while keeping application data and settings independent.

## Current status

**Version 0.2.0.** The Windows CLI manages FFmpeg and SoX. Applications can be registered and linked to shared tools, and resolved paths are checked against recorded SHA-256 hashes. SoX uses a pinned 14.4.2 archive and validates its SHA-1 checksum before extraction. The FFmpeg endpoint is still a rolling release and does not yet have a pinned upstream checksum.

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
    ├── ffmpeg/
    │   └── gyan-release-essentials/
    │       ├── ffmpeg.exe
    │       └── ffprobe.exe
    └── sox/
        └── 14.4.2/
            └── sox-14.4.2/
                ├── sox.exe
                └── supporting DLLs
```

The exact SoX subfolder follows the archive layout. The manager does not add global PATH entries and does not modify application folders.

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

Install and link shared tools:

```powershell
srm install ffmpeg --app ttk-media
srm install sox --app ttk-media
srm install ffmpeg --app nova-generator
srm install sox --app nova-generator
```

Resolve a resource path for an application:

```powershell
srm resolve ffmpeg --app ttk-media
srm resolve sox --app ttk-media
```

Other commands:

```powershell
srm root
srm apps
srm resources
```

The returned path is the directory containing the tool executable. For SoX, run `sox.exe` from that directory. The manager intentionally does not change PATH.

## Safety model

- Downloads are cached and SoX is verified against a pinned SHA-1 checksum published by the SoX developers for the 14.4.2 Windows ZIP release.
- ZIP paths are checked before extraction, and SoX symbolic-link entries are skipped.
- Installations are extracted to a staging directory. Existing installations are moved to a backup and restored if the replacement move fails.
- The registry records SHA-256 hashes for installed executable files and SoX runtime files.
- A resource is only resolved after its registered files pass integrity checks.
- Applications are linked explicitly in the registry.
- There is intentionally no uninstall command yet. Removal must first check all application references.

**Security note:** SHA-1 is used only to match the historical upstream SoX release checksum. SHA-256 is used for local post-install integrity checks. SoX 14.4.2 is an old release, and the Windows package is x86. A newer maintained x64 build should be evaluated before using SoX for a production distribution. FFmpeg still uses Gyan.dev's rolling endpoint and needs a pinned release/checksum before the installer can be considered supply-chain hardened.

## Roadmap

1. Windows FFmpeg and SoX management with application links.
2. Version-aware resource records and safe removal with reference checks.
3. Pinned Python and Node.js runtimes.
4. Shared model storage with per-application references.
5. Tauri integration and a small desktop management screen.
