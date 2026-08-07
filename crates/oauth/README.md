# kimi-code-oauth

`kimi-code-oauth` provides the OAuth and managed authentication layer for Kimi
Code. It implements the device authorization flow, token persistence and
refresh, Kimi host identity headers, and managed provider integration.

## Features

- OAuth device authorization, polling, access-token refresh, and structured
  API errors.
- `OAuthManager` with cached-token access, single-flight refresh, cross-process
  refresh coordination, login cancellation, and refresh observers.
- `KimiOAuthToolkit` as the high-level API for authentication status, login,
  logout, fresh access tokens, usage, and feedback operations.
- Pluggable asynchronous token storage through the `TokenStorage` trait, with a
  filesystem-backed default implementation.
- Managed Kimi Code authentication, configuration provisioning, model
  discovery, usage reporting, and feedback uploads.
- Open-platform and custom-registry model discovery and configuration helpers.
- Device identity, user-agent, default header, token-state, and model-alias
  utilities.

Network and filesystem operations are asynchronous. The default implementation
uses Tokio and Reqwest with Rustls.

## Usage

Add the crate to a workspace member's `Cargo.toml`:

```toml
[dependencies]
kimi-code-oauth = { path = "../oauth" }
```

Create the default toolkit and inspect the managed Kimi Code login state from
an async context:

```rust
use kimi_code_oauth::{
    KimiOAuthToolkit, KimiOAuthToolkitOptions, NoManagedConfigAdapter,
};

let toolkit = KimiOAuthToolkit::<NoManagedConfigAdapter>::new(
    KimiOAuthToolkitOptions::default(),
)?;
let status = toolkit.status(None, None).await?;

assert_eq!(status.providers.len(), 1);
```

The default toolkit stores credentials below
`~/.kimi-code/credentials`. Supply a custom `home_dir`, `credentials_dir`, or
`TokenStorage` implementation through `KimiOAuthToolkitOptions` when the host
application owns storage policy.

## Configuration

The default Kimi Code integration recognizes these environment overrides:

- `KIMI_CODE_HOME`: Kimi Code data directory.
- `KIMI_CODE_OAUTH_HOST`: OAuth service host.
- `KIMI_OAUTH_HOST`: legacy OAuth host fallback.
- `KIMI_CODE_BASE_URL`: managed Kimi Code API base URL.
- `KIMI_CODE_CUSTOM_HEADERS`: additional identity headers.

Explicit values supplied through toolkit and operation options take precedence
where the corresponding API provides an override.

## Modules

- `toolkit`: high-level authentication and managed-service API.
- `manager`: token lifecycle, login, refresh coordination, and logout.
- `flow`: device authorization, polling, and refresh HTTP operations.
- `storage`: the `TokenStorage` abstraction and `FileTokenStorage`.
- `types`, `token_state`, and `errors`: OAuth values and error types.
- `identity`: device ID, user-agent, and Kimi request headers.
- `managed_auth`, `managed_config`, and `managed_provision`: managed Kimi Code
  credentials and host configuration.
- `managed_models`, `managed_usage`, `managed_userinfo`, `managed_feedback`,
  and `managed_feedback_upload`: managed service APIs.
- `open_platform`, `custom_registry`, and `refresh_provider_models`: external
  provider model discovery and configuration.
- `model_alias_merge`: preservation of host-owned model alias fields during
  refresh.

The crate root re-exports the public API from these modules.

## Security

Treat `TokenInfo`, access tokens, refresh tokens, device codes, and generated
authorization headers as secrets. Do not log them or include them in user-facing
errors. `FileTokenStorage` validates token names and writes credentials through
its controlled storage directory, but the host application remains responsible
for directory access policy and backups.

## Validation

```shell
cargo test -p kimi-code-oauth
cargo clippy -p kimi-code-oauth --all-targets -- -D warnings
```
