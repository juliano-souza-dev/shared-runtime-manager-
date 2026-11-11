# Shared Runtime Manager

A shared dependency, runtime, model-storage, and API-configuration manager for JulianoDev desktop applications.

The design goal is one active shared installation/configuration per resource, consumed by explicitly authorized applications. Applications must not maintain private copies of centrally managed dependencies or credentials.

## Current status

**Version 0.3.0.** The Windows CLI manages FFmpeg and SoX and now supports global API configuration metadata plus secrets stored in Windows Credential Manager. The API registry contains no API keys. Application authorization is explicit.

The API feature is an initial centralized storage layer. Direct application SDK integration, compatibility declarations, process-use leases, coordinated dependency updates, and a local inference service remain future work.

## Shared location

On Windows, data is stored under:

```text
%LOCALAPPDATA%\JulianoDev\SharedRuntime\
├── cache/
├── models/
├── registry/
│   ├── state.json
│   └── apis.json
├── runtimes/
└── tools/
    ├── ffmpeg/
    └── sox/
```

API secrets are stored separately in **Windows Credential Manager**, under service `JulianoDev.SharedRuntime.Api` and account `api:<provider-id>`. They are not written to JSON, command-line arguments, or application configuration files.

## Build

Install the Rust stable toolchain, then:

```powershell
cargo build --release
cargo test --all-targets
```

The executable is `target/release/srm.exe` on Windows.

## Register applications

```powershell
srm register ttk-media --name "TTK Media" --path "C:\\Projects\\ttk-media"
srm register nova-generator --name "Nova Generator" --path "C:\\Projects\\nova-generator"
```

Each registered application must provide its absolute installation directory. Shared Runtime creates or updates a `shared.config.json` file in that directory. The file contains the application identity and absolute paths to the shared root, managed directories, registries, and resources explicitly linked to that application.

Example:

```powershell
srm register nova-generator --name "Nova Generator" --path "C:\\Projects\\nova-generator"
```

The generated file is the application-to-Shared contract, not a secret store. The application owns the declarations in `shared.dependencies` and `shared.providers`; Shared Runtime preserves them during registration and every resource refresh. Shared Runtime owns and refreshes `enabled`, `root`, `directories`, `files`, and `resources`. API keys remain in Windows Credential Manager. Invalid JSON or invalid declaration structures are not overwritten.

```json
{
  "schemaVersion": "1.0",
  "application": {
    "id": "ttk-media",
    "name": "TTK Media"
  },
  "shared": {
    "enabled": true,
    "dependencies": [
      {
        "id": "ffmpeg",
        "required": true,
        "version": ">=7"
      },
      {
        "id": "yt-dlp",
        "required": true
      },
      {
        "id": "python",
        "required": true
      },
      {
        "id": "faster-whisper",
        "required": true,
        "version": ">=1.1,<2"
      },
      {
        "id": "pyav",
        "required": true,
        "version": ">=11,<19"
      },
      {
        "id": "whisper-models",
        "required": true
      }
    ],
    "providers": [
      {
        "id": "groq",
        "required": false
      }
    ],
    "root": "C:\\Users\\User\\AppData\\Local\\JulianoDev\\SharedRuntime",
    "directories": {
      "tools": "...\\tools",
      "runtimes": "...\\runtimes",
      "models": "...\\models",
      "cache": "...\\cache",
      "registry": "...\\registry"
    },
    "files": {
      "applications": "...\\registry\\state.json",
      "apiProviders": "...\\registry\\apis.json"
    },
    "resources": {}
  }
}
```

### Configuration ownership and validation

The application declares the resources it needs in `shared.dependencies` and the external API providers it may use in `shared.providers`. Each entry requires a unique `id`. The optional `required` field is boolean, and the optional `version` field is a version constraint for dependencies.

Example dependency: `{"id":"ffmpeg","required":true,"version":">=7"}`. Example provider: `{"id":"groq","required":false}`.

The Shared Runtime Manager validates that both properties are arrays, every entry has a safe identifier, IDs are unique within each list, and optional `required` and `version` values have the correct types. Missing lists are initialized as empty arrays. Malformed declarations are not silently replaced.

Ownership is deliberately split:
- **Application-owned:** `application`, `shared.dependencies`, and `shared.providers`.
- **Shared-owned:** `schemaVersion`, `shared.enabled`, `shared.root`, `shared.directories`, `shared.files`, and `shared.resources`.

`shared.resources` contains resolved absolute paths for resources actually installed and linked to that application. Declaring a dependency does not automatically install or link it yet; automatic dependency resolution/installation is a separate next capability. Provider declarations do not store API keys or grant authorization by themselves. Provider credentials and per-app authorization remain in the global API registry and Windows Credential Manager.

Applications should resolve their shared paths from this file at startup. They must not hardcode the Shared Runtime installation directory or copy centrally managed resources into their own project folders.

## Shared dependencies

```powershell
srm install ffmpeg --app ttk-media
srm install sox --app ttk-media
srm resolve ffmpeg --app ttk-media
```

## Global API configuration

Create or update a provider's shared non-secret settings:

```powershell
srm api add groq --name "Groq" --endpoint "https://api.groq.com/openai/v1" --model "llama-3.3-70b-versatile"
```

Authorize applications individually:

```powershell
srm api authorize groq --app ttk-media
srm api authorize groq --app nova-generator
```

Set the key through standard input. This avoids putting the secret in shell command arguments or process command-line listings:

```powershell
Read-Host "Groq API key" -AsSecureString | ForEach-Object {
    $ptr = [Runtime.InteropServices.Marshal]::SecureStringToBSTR($_)
    try {
        $plain = [Runtime.InteropServices.Marshal]::PtrToStringBSTR($ptr)
        $plain | srm api set-key groq
    } finally {
        [Runtime.InteropServices.Marshal]::ZeroFreeBSTR($ptr)
    }
}
```

List providers and check credential presence:

```powershell
srm api list
srm api key-status groq
```

Resolve settings for an authorized application. The output contains endpoint/model metadata and authorized application IDs, **never the API key**:

```powershell
srm api resolve groq --app ttk-media
```

### Application integration contract

Windows desktop applications should use the same OS credential-store entry when retrieving a secret:

- Credential service: `JulianoDev.SharedRuntime.Api`
- Credential account: `api:<provider-id>`
- Secret storage: Windows Credential Manager
- Non-secret configuration: `registry/apis.json`
- Authorization: the application's ID must be present in that provider's `authorized_apps` list.

The application must not copy the key into its own settings file, logs, crash reports, or command-line arguments. The Rust manager's internal accessor checks the authorization registry before returning a secret. A stable public SDK/IPC interface is planned so applications do not need to implement credential-store access independently.

## Security and lifecycle notes

- API keys are stored through the Windows native credential backend provided by the keyring crate.
- API metadata and authorization lists are stored in the shared registry; secrets are not.
- The CLI never prints API secrets.
- Applications are authorized per provider.
- Do not pass API keys as command-line arguments. Use standard input as shown above.
- Credential storage is currently Windows-only. Other platforms fail closed rather than writing secrets in plaintext.
- This first API implementation does not yet provide a background broker/IPC service, credential rotation notifications, or audit events.
- The current SoX 14.4.2 package is an old Windows x86 build and needs replacement with a maintained x64 distribution before production use.
- FFmpeg currently uses Gyan.dev's rolling endpoint and still needs a pinned release/checksum for stronger supply-chain verification.

## Architecture roadmap

1. Shared API configuration and OS-protected credentials (initial CLI implementation).
2. A stable application SDK/IPC contract for retrieving authorized settings and secrets.
3. Compatibility registry and a single-active-installation update coordinator for shared runtimes.
4. Application-use leases/process coordination to prevent replacing binaries while in use.
5. Shared model storage and, where RAM/VRAM sharing is required, a central inference service that loads a model once and serves multiple applications.


## Desktop interface (0.4.0)

The desktop management interface is built with Tauri 2 and uses the same Rust library as the CLI. It provides:

- Dashboard with registered applications, API providers, shared resources, and the local data directory.
- Visual application registration.
- Global API provider configuration.
- Per-application API authorization and revocation.
- API key updates through the Windows Credential Manager.
- Shared FFmpeg and SoX installation with optional application linking.

Build the CLI and run the desktop app on Windows:

```powershell
cargo build --release
cargo run --release --features desktop --bin srm-gui
```

The GUI feature is optional, so the existing CLI and its CI checks remain independent of the desktop runtime. The desktop interface currently targets Windows because protected API credential storage is Windows-only. API keys are entered into a password field and passed directly to the native credential store; they are never returned to the UI or written to the shared JSON registry.

The first desktop release is a local management interface, not a background Windows service. Application discovery, signed app identity, compatibility approvals, update coordination, and a brokered SDK/IPC remain separate roadmap items.
