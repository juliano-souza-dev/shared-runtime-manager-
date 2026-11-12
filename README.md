# Shared Runtime Manager

A shared dependency, runtime, model-storage, and API-configuration manager for JulianoDev desktop applications.

The design goal is one active shared installation/configuration per resource, consumed by explicitly authorized applications. Applications must not maintain private copies of centrally managed dependencies or credentials.

## Current status

**Version 0.5.0.** Shared Runtime manages local dependencies and provider credentials. API keys are protected with Windows DPAPI, stored in the local Shared registry, and provisioned in encrypted form only to each authorized application's `shared.config.json`. Application authorization is explicit.

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

API secrets are encrypted with **Windows DPAPI** for the current Windows user. The encrypted credential is stored in the local API registry and copied into the `shared.apiProviders` section of each authorized application's `shared.config.json`. Plaintext keys are never written to disk, command-line arguments, or logs.

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

The generated file is the application-to-Shared contract. The application owns the declarations in `shared.dependencies` and `shared.providers`; Shared Runtime preserves them during registration and every resource refresh. Shared Runtime owns and refreshes `enabled`, `root`, `directories`, `files`, `resources`, and `apiProviders`. The latter contains only the providers authorized for that application, with DPAPI-encrypted credentials. Invalid JSON or invalid declaration structures are not overwritten.

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
- **Shared-owned:** `schemaVersion`, `shared.enabled`, `shared.root`, `shared.directories`, `shared.files`, `shared.resources`, and `shared.apiProviders`.

`shared.resources` contains resolved absolute paths for resources actually installed and linked to that application. `shared.providers` remains an application-owned declaration list and does not grant access by itself. Actual authorization is managed centrally; Shared materializes the authorized provider settings and encrypted credential in `shared.apiProviders`.

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

Applications consume provider settings from their own generated `shared.config.json`; the Shared desktop application does not need to remain open.

The Rust library exposes these functions:

- `providers_for_app(app_id)`: obtains the encrypted providers provisioned for an application.
- `get_provider_settings_from_config(config_path, app_id, provider_id)`: loads one provider and verifies the application identity.
- `get_provider_credential_from_config(config_path, app_id, provider_id)`: decrypts the credential for backend use.
- `get_authorized_providers_from_config(config_path, app_id)`: lists the provisioned providers without exposing plaintext keys.

The credential is encrypted with Windows DPAPI for the current Windows user. Each application should retrieve it only in its Rust backend and must never send it to the frontend, logs, crash reports, or command-line arguments. DPAPI protects credentials at rest; it is not an isolation boundary against malicious processes running as the same Windows user.

## Security and lifecycle notes

- API keys are encrypted with Windows DPAPI before being written to local JSON files.
- The local API registry is the source of truth; each application's config receives only the providers authorized for that application.
- Existing credentials in Windows Credential Manager are migrated to DPAPI-encrypted storage when the registry is next read.
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
- API key updates using Windows DPAPI and per-application config provisioning.
- A dependency library that lists centrally installed resources across all applications.
- Provider presets for Groq, OpenAI, Anthropic, Google Gemini, DeepSeek, Mistral, and OpenRouter.
- Live model discovery from each provider after its API key is stored.

Build the CLI and run the desktop app on Windows:

```powershell
cargo build --release
cargo run --release --features desktop --bin srm-gui
```

The GUI feature is optional, so the existing CLI and its CI checks remain independent of the desktop runtime. The desktop interface currently targets Windows because DPAPI-protected API credential storage is Windows-only. API keys are entered into a password field, encrypted by the backend, and never returned to the UI in plaintext.

The first desktop release is a local management interface, not a background Windows service. Application discovery, signed app identity, compatibility approvals, update coordination, and a brokered SDK/IPC remain separate roadmap items.


## Application dependency contract and first registration

When an application is registered with an absolute path, Shared creates or updates its root-level `shared.config.json`, adds the declared dependencies to a durable queue, and returns control without waiting for downloads. The queue installs each unique dependency centrally, then refreshes the configs of every registered application that declares it. Individual dependency failures are recorded without preventing the remaining dependencies from being processed. If the `dependencies` array is absent, Shared writes this default contract:

```json
{
  "dependencies": [
    { "id": "ffmpeg", "required": true },
    { "id": "ffprobe", "required": true },
    { "id": "yt-dlp", "required": true },
    { "id": "python", "required": true },
    { "id": "faster-whisper", "required": true },
    { "id": "pyav", "required": true },
    { "id": "whisper-models", "required": true }
  ]
}
```

The dependency IDs are independent. FFmpeg and FFprobe may arrive in the same archive, but each is checked and recorded separately. An already available executable on PATH or an existing Shared installation is reused. Python packages are checked by importing their module before pip is invoked. Existing versions are not compared, upgraded, replaced, or duplicated by Shared.

Provisioned shared dependency paths are written to `shared.dependencies`. Application-owned directories are created below the application's own `data/` directory and exposed as `application.data`: `projects`, `database`, `sounds`, `logs`, and `user-files`. These directories are never shared between applications.

Groq is a cloud provider, not a downloadable local dependency. Its provider settings and DPAPI-encrypted credential are stored in the local API registry and provisioned into each authorized application's `shared.config.json`.

Python is reused when present. On Windows, if Python is missing and Windows Package Manager is available, Shared requests Python 3.12 through winget. Missing Faster-Whisper and PyAV Python packages are installed into the selected Python environment only after import checks show they are absent.

The `whisper-models` dependency currently provisions the shared model-storage directory. Actual model weights are downloaded by the consuming application's model-selection/loading flow, because the application must choose which Whisper model size it needs. Shared does not guess a model size or download every model variant.


## Dependency library and API provider catalog

The desktop page **Biblioteca de dependências** is an inventory of resources resolved in the central Shared Runtime directory. It combines the legacy resource registry with `registry/resolved-dependencies.json`, so an installation is listed regardless of which application first requested it. Resource installation is part of application registration and queue processing; the desktop library is intentionally read-only.

Provider presets are bundled in `assets/api-providers.json` and compiled into the application. Each entry documents the provider ID, display name, base endpoint, model-list endpoint, authentication style, API-key creation page, official documentation, and initial model suggestions. The current presets cover Groq, OpenAI, Anthropic, Google Gemini, DeepSeek, Mistral AI, and OpenRouter.

The provider catalog is metadata, not an installer: cloud APIs are accessed over HTTPS and require an API key. The UI pre-fills the provider name and endpoint. After the key is stored in the local encrypted API registry, **Modelos** calls the provider's model-list endpoint and offers the models currently returned by that account. The bundled model suggestions are only an initial fallback; the live provider response is authoritative. Authentication differs by provider: Bearer token for OpenAI-compatible providers, Anthropic API-key/version headers for Claude, and a query-string API key for Gemini.


## Synchronizing registered applications and removing dependencies

The desktop **Applications** page provides a **Synchronize dependencies** action for every registered application. It re-reads the application's existing `shared.config.json`, preserves its dependency declarations, queues the required resources, reuses binaries already found on PATH or in Shared, and refreshes the resolved dependency links. If the configuration file is missing, Shared recreates its generated configuration before queueing the synchronization.

The application list shows dependencies declared by the project as well as dependencies already resolved in its configuration. The dependency library identifies which applications still reference each dependency.

A dependency cannot be deleted while a registered application declares or resolves it, while a legacy application link exists, or while it is waiting in the installation queue. The backend enforces these checks even if the UI is bypassed.

Deletion is intentionally limited to files managed inside the central Shared Runtime directory. Shared can remove its own FFmpeg/FFprobe binaries, yt-dlp installation, and Whisper model directory. Dependencies discovered outside Shared (for example, Python or tools already installed elsewhere on PATH) are marked as external and are not deleted from the user's system.
