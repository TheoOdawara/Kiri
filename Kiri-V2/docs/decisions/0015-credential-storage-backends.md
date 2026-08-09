# 0015. General credential storage backends

- Status: accepted
- Date: 2026-08-09

## Context

Kiri-V2 needs one credential boundary for provider API keys and future secret
values. Local keyless providers do not need a stored credential, but remote
providers must not place raw tokens in configuration, resources, sessions, or
logs.

The default must work on a fresh installation without requiring an operating
system credential manager. Users who prefer native secret storage must be able
to switch to a keyring without changing provider profiles.

## Decision

Kiri-V2 uses a general `CredentialStore` with two selectable backends:

- `file` is the default backend;
- `keyring` uses the operating system's native credential manager when
  available.

The global configuration selects the backend with `[credentials].backend`.
Project configuration cannot override it.

Provider profiles contain only an opaque `credential` reference. The credential
value is stored by the selected backend. Local keyless providers omit the
reference entirely and never create a credential entry.

The file backend stores values in the Kiri-owned global credential file
`~/.kiri/credentials.json`, with owner-only permissions where the operating
system supports them, atomic writes, and redacted diagnostics. The keyring
backend stores the same references and values in the native OS credential
manager under Kiri's service namespace.

Changing the backend is a migration:

1. copy every existing credential entry to the destination backend;
2. verify that every destination entry can be read back;
3. atomically persist the new active backend;
4. remove source entries only after the new backend is active.

If any step before activation fails, the current backend remains active and no
provider profile is changed. Kiri never silently falls back from `keyring` to
`file`, or from `file` to `keyring`, when the selected backend is unavailable.

Credential references remain stable across backend migrations. Raw credential
values are never valid configuration fields and are never written to logs,
sessions, resource bodies, or model context.

## Consequences

- Fresh installations work with file storage without requiring a keyring.
- Users can switch storage protection without editing every provider profile.
- Backend availability and migration failures are explicit and recoverable.
- The file backend must enforce local file permissions and atomicity on every
  supported operating system.
- The keyring backend requires platform-specific adapters but does not change
  the application-facing credential contract.

## Alternatives considered

- **Keyring-only storage:** rejected because a fresh installation must work on
  systems without a usable native credential manager.
- **File-only storage:** rejected because users need an optional native secure
  storage path.
- **A separate backend per provider:** rejected because it fragments policy and
  makes migration and diagnostics inconsistent.
- **Silent fallback between backends:** rejected because it can write a secret
  to a storage location the user did not select.
