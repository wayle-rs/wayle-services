# wayle-mullvad

Reactive [Mullvad VPN](https://mullvad.net) status and control for Wayle.

The crate exposes a small, stable public API over the Mullvad daemon's gRPC
management interface. State is published as reactive
[`Property`](https://docs.rs/wayle-core) fields, and the tunnel is driven with
async `connect`/`disconnect` methods.

## Public API

The [`MullvadService`] owns lifecycle; reactive state and controls live on its
`mullvad` model:

- `logged_in: Property<bool>` — whether an account is logged in
- `connection_state: Property<ConnectionState>` — `Disconnected` / `Connecting`
  / `Connected` / `Disconnecting` / `Error`
- `connected_network: Property<Option<ConnectedNetwork>>` — the current relay's
  hostname and location
- `networks: Property<Vec<NetworkCountry>>` — the available networks as a
  country → city → network tree
- `connect(&NetworkTarget)` / `disconnect()` — drive the tunnel

```rust,no_run
use wayle_mullvad::{MullvadService, NetworkTarget};

# async fn example() -> Result<(), wayle_mullvad::Error> {
let service = MullvadService::new().await?;

// Observe reactive state.
let _logged_in = service.mullvad.logged_in.get();

// Connect to Sweden, then to a specific city, then disconnect. These are
// non-blocking; the result shows up in the reactive state.
service.mullvad.connect(&NetworkTarget::country("se"));
service.mullvad.connect(&NetworkTarget::city("se", "got"));
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
