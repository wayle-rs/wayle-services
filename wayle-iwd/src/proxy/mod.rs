//! Hand-written zbus proxies for the `net.connman.iwd` D-Bus interfaces.

/// Adapter proxy (`net.connman.iwd.Adapter`).
pub(crate) mod adapter;
/// Agent manager proxy (`net.connman.iwd.AgentManager`).
pub(crate) mod agent_manager;
/// Per-station diagnostics proxy (`net.connman.iwd.StationDiagnostic`).
pub(crate) mod diagnostic;
/// Device proxy (`net.connman.iwd.Device`).
pub(crate) mod device;
/// Known network proxy (`net.connman.iwd.KnownNetwork`).
pub(crate) mod known_network;
/// Network proxy (`net.connman.iwd.Network`).
pub(crate) mod network;
/// `org.freedesktop.DBus.ObjectManager` proxy scoped to `net.connman.iwd`.
pub(crate) mod object_manager;
/// Station proxy (`net.connman.iwd.Station`).
pub(crate) mod station;
