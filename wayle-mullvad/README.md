# wayle-mullvad

Reactive [Mullvad VPN](https://mullvad.net) status and control for Wayle.

The crate exposes a small, stable public API over the Mullvad daemon's gRPC
management interface. State is published as reactive
[`Property`](https://docs.rs/wayle-core) fields, and the tunnel is driven with
async `connect`/`disconnect` methods.

## Public API

The [`MullvadService`] owns lifecycle; reactive state and controls live on its
`mullvad` model:

- `status: Property<ConnectionStatus>` — the overall VPN status:
  `LoggedOut` / `Revoked` / `Disconnected` / `Connecting(relay)` /
  `Connected(relay)` / `Disconnecting` / `Error(cause)`. Folds the tunnel state,
  the active relay, and the account login state (the latter taking precedence)
  into one value so illegal combinations can't occur
- `selected: Property<Option<RelayLocation>>` — the relay the *daemon* has
  selected, i.e. the one `connect()` would use (display names). Sourced
  authoritatively from the daemon's persisted relay settings (snapshot + settings
  events), so it reflects what connect will do even before this client calls
  `select()`. Orthogonal to `status`; `None` when the daemon has no single
  geographic selection (custom list / unconstrained)
- `networks: Property<Vec<NetworkCountry>>` — the available networks as a
  country → city → network tree
- `select(&NetworkTarget)` — choose the relay location (persisted, no connect)
- `connect()` / `disconnect()` — drive the tunnel to the selection

```rust,no_run
use wayle_mullvad::{MullvadService, NetworkTarget};

# async fn example() -> Result<(), wayle_mullvad::Error> {
let service = MullvadService::new().await?;

// Observe reactive state (status folds in the login state).
let _status = service.mullvad.status.get();

// Select Sweden and connect, then reselect a specific city, then disconnect.
// These are non-blocking; the result shows up in the reactive state.
service.mullvad.select(&NetworkTarget::country("se"));
service.mullvad.connect();
service.mullvad.select(&NetworkTarget::city("se", "got"));
service.mullvad.disconnect();
# Ok(())
# }
```

## Versioned backends

The daemon's management interface is an internal, unversioned gRPC API whose
wire schema changes between releases. To stay robust across versions, the schema
is confined behind the `MullvadBackend` trait. On startup the service queries
the daemon version (via a tiny, version-independent bootstrap client) and picks
a backend from a registry mapping version ranges to implementations. Unsupported
versions produce a clear `Error::UnsupportedVersion`.

To add support for a new daemon version:

1. add a backend under `src/backends/<version>/` (its minimal `.proto` subset and
   a `MullvadBackend` implementation),
2. add a `BackendKind` variant,
3. register its version range in the `REGISTRY` in `src/backend.rs`, and
4. add the matching arm in `connect_backend`.

## Build requirements

The backend schemas are compiled at build time with a vendored `protoc`
(via `protoc-bin-vendored`), so no system protobuf compiler is required.
