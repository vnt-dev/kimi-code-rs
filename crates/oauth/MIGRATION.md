# OAuth migration status

Source package in the original repository: `packages/oauth`

## Production source coverage

| TypeScript source | Rust counterpart | Status |
| --- | --- | --- |
| `src/api-error.ts` | `src/api_error.rs` | Migrated |
| `src/constants.ts` | `src/constants.rs` | Migrated |
| `src/custom-registry.ts` | `src/custom_registry.rs` | Migrated |
| `src/errors.ts` | `src/errors.rs` | Migrated |
| `src/identity.ts` | `src/identity.rs` | Migrated |
| `src/index.ts` | `src/lib.rs` | Migrated |
| `src/managed-feedback-upload.ts` | `src/managed_feedback_upload.rs` | Migrated |
| `src/managed-feedback.ts` | `src/managed_feedback.rs` | Migrated |
| `src/managed-kimi-code.ts` | `src/managed_auth.rs`, `managed_config.rs`, `managed_models.rs`, `managed_provision.rs` | Migrated and split by responsibility |
| `src/managed-usage.ts` | `src/managed_usage.rs` | Migrated |
| `src/model-alias-merge.ts` | `src/model_alias_merge.rs` | Migrated |
| `src/oauth-manager.ts` | `src/manager.rs` | Migrated |
| `src/oauth.ts` | `src/oauth.rs` (exported as `flow`) | Migrated |
| `src/open-platform.ts` | `src/open_platform.rs` | Migrated |
| `src/refreshProviderModels.ts` | `src/refresh_provider_models.rs` | Migrated |
| `src/storage.ts` | `src/storage.rs` | Migrated |
| `src/token-state.ts` | `src/token_state.rs` | Migrated |
| `src/toolkit.ts` | `src/toolkit.rs`, `src/home.rs` | Migrated |
| `src/types.ts` | `src/types.rs` | Migrated |
| `src/utils.ts` | Record-shape checks are expressed with `serde_json::Value` matching in their consumers | Migrated inline |

No `MIGRATION-TODO`, `todo!`, `unimplemented!`, empty production function, or fake production return value was found during this audit.

Rust-specific adaptations include typed error enums in place of JavaScript error subclasses, traits in place of callback-shaped interfaces, an owned flow configuration constructor in place of the JavaScript object constant, and a directory/heartbeat refresh lock in place of `proper-lockfile`.

## Verification coverage

- Rust OAuth crate: 140 tests.
- Original TypeScript OAuth package: 254 `it(...)`/`test(...)` declarations.
- The Rust tests consolidate many TypeScript cases, so production API coverage is complete but one-to-one test-vector parity is not yet proven.
- The original TypeScript suite was not run during this audit because the source checkout has no `node_modules`, `pnpm` is unavailable, and the installed Node.js 22 runtime is below the monorepo's required Node.js 24.15.

## Non-production package files

The TypeScript package's README, changelog, TypeScript build configuration, and smoke-test example are not copied into this Rust crate. They do not affect runtime behavior, but equivalent Rust crate documentation and an OAuth smoke example remain optional follow-up work.
