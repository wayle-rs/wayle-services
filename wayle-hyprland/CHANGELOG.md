# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.3.1](https://github.com/wayle-rs/wayle-services/compare/wayle-hyprland-v0.3.0...wayle-hyprland-v0.3.1) - 2026-11-02

### Other

- release ([#42](https://github.com/wayle-rs/wayle-services/pull/42))

## [0.3.0](https://github.com/wayle-rs/wayle-services/compare/wayle-hyprland-v0.2.3...wayle-hyprland-v0.3.0) - 2026-11-02

### Added

- *(wayle-hyprland)* [**breaking**] support Hyprland 0.57 workspace addresses ([#55](https://github.com/wayle-rs/wayle-services/pull/55))

### Migrating

Hyprland 0.57 identifies workspaces by address instead of by number, and named and special workspaces no longer have an ID. 0.3.0 reads both Hyprland 0.56 and 0.57.

1. `Workspace::id` and `WorkspaceInfo::id` are now `Option<WorkspaceId>`. Only numbered workspaces have one. To identify a workspace, use the new `address` field, and look workspaces up with `HyprlandService::workspace_by_address`. `HyprlandService::workspace(id)` still works for numbered workspaces.

   ```rust
   let workspace = service.workspace(id).await;
   ```

   becomes:

   ```rust
   let workspace = service.workspace_by_address(&address).await;
   ```

2. `Monitor::special_workspace` is now `Property<Option<WorkspaceInfo>>`. It is `None` when no special workspace is open on the monitor. On Hyprland 0.56, 0.2 returned a placeholder with ID 0 and an empty name instead.

   ```rust
   let special_open = !monitor.special_workspace.get().name.is_empty();
   ```

   becomes:

   ```rust
   let special_open = monitor.special_workspace.get().is_some();
   ```

3. Workspace events carry the address next to the optional ID:
   - `WorkspaceV2`, `CreateWorkspaceV2`, `DestroyWorkspaceV2`, `MoveWorkspaceV2` and `RenameWorkspace`: `id` is now `Option<WorkspaceId>`, and a new `address` field holds the workspace address.
   - `FocusedMonV2` and `MoveWindowV2`: `workspace_id` is now `Option<WorkspaceId>`, and a new `workspace_address` field holds the address.
   - `ActiveSpecialV2`: `id` is removed. Use `address`, which is `None` when the special workspace closes.
   - New `ChangeWorkspaceId { old_id, new_id }`, sent when Hyprland renumbers a workspace.

   ```rust
   if let HyprlandEvent::WorkspaceV2 { id, name } = event {}
   ```

   becomes:

   ```rust
   if let HyprlandEvent::WorkspaceV2 { address, name, .. } = event {}
   ```

4. `HyprlandEvent`, `Error`, `Workspace`, `WorkspaceInfo`, `Monitor` and `Client` are now `#[non_exhaustive]`. A `match` on `HyprlandEvent` or `Error` that lists every variant needs a `_` arm, and struct patterns on these types need `..`.

## [0.2.3](https://github.com/wayle-rs/wayle-services/compare/wayle-hyprland-v0.2.2...wayle-hyprland-v0.2.3) - 2026-07-22

### Fixed

- *(hyprland)* accept allowedOverFullscreen client field ([#40](https://github.com/wayle-rs/wayle-services/pull/40))

## [0.2.2](https://github.com/wayle-rs/wayle-services/compare/wayle-hyprland-v0.2.1...wayle-hyprland-v0.2.2) - 2026-06-16

### Fixed

- use lossy utf8 decoding for Hyprland ipc events ([#31](https://github.com/wayle-rs/wayle-services/pull/31))

## [0.2.1](https://github.com/wayle-rs/wayle-services/compare/wayle-hyprland-v0.2.0...wayle-hyprland-v0.2.1) - 2026-05-18

### Other

- *(wayle-hyprland)* release v0.2.0 ([#18](https://github.com/wayle-rs/wayle-services/pull/18))

## [0.2.0](https://github.com/wayle-rs/wayle-services/compare/wayle-hyprland-v0.1.2...wayle-hyprland-v0.2.0) - 2026-05-12

### Added

- [**breaking**] added eval hyprctl command and removed the keyword command ([#17](https://github.com/wayle-rs/wayle-services/pull/17))

## [0.1.2](https://github.com/wayle-rs/wayle-services/compare/wayle-hyprland-v0.1.1...wayle-hyprland-v0.1.2) - 2026-04-03

### Other

- switch to independent crate versioning ([#7](https://github.com/wayle-rs/wayle-services/pull/7))

## [0.1.1](https://github.com/wayle-rs/wayle-services/compare/wayle-hyprland-v0.1.0...wayle-hyprland-v0.1.1) - 2026-03-30

### Other

- readme and doc updates (plus willi the whale :])
