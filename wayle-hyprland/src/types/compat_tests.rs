//! Parses saved `hyprctl -j` responses from each supported Hyprland version
//! and checks that both normalize to the same workspace identities.
//!
//! The 0.57 fixtures are captured from a live session with personal data
//! scrubbed. The 0.56 fixtures are the same data rewritten into the 0.56
//! workspace format (`id` only, negative IDs for named and special
//! workspaces, `{"id": 0, "name": ""}` for an empty special slot).

use serde_json::Error as JsonError;

use crate::{
    ClientData, MonitorData, WorkspaceAddress, WorkspaceData, WorkspaceId, WorkspaceInfo,
    WorkspaceKind,
};

const MONITORS_0_56: &str = include_str!("../../tests/fixtures/hyprland-0.56/monitors.json");
const MONITORS_0_57: &str = include_str!("../../tests/fixtures/hyprland-0.57/monitors.json");
const WORKSPACES_0_56: &str = include_str!("../../tests/fixtures/hyprland-0.56/workspaces.json");
const WORKSPACES_0_57: &str = include_str!("../../tests/fixtures/hyprland-0.57/workspaces.json");
const CLIENTS_0_56: &str = include_str!("../../tests/fixtures/hyprland-0.56/clients.json");
const CLIENTS_0_57: &str = include_str!("../../tests/fixtures/hyprland-0.57/clients.json");

type Summary = (Option<WorkspaceId>, WorkspaceAddress, WorkspaceKind);
type MonitorSummary = (Summary, Option<Summary>);

fn summarize(workspace: &WorkspaceInfo) -> Summary {
    (workspace.id, workspace.address.clone(), workspace.kind)
}

fn monitor_workspaces(json: &str) -> Result<Vec<MonitorSummary>, JsonError> {
    let monitors: Vec<MonitorData> = serde_json::from_str(json)?;

    let summaries = monitors
        .iter()
        .map(|monitor| {
            let active = summarize(&monitor.active_workspace);
            let special = monitor.special_workspace.as_ref().map(summarize);

            (active, special)
        })
        .collect();

    Ok(summaries)
}

fn workspace_list(json: &str) -> Result<Vec<Option<Summary>>, JsonError> {
    let workspaces: Vec<WorkspaceData> = serde_json::from_str(json)?;

    let summaries = workspaces
        .iter()
        .map(|workspace| {
            let identity = workspace.identity()?;

            Some((identity.id, identity.address, identity.kind))
        })
        .collect();

    Ok(summaries)
}

fn client_workspaces(json: &str) -> Result<Vec<Summary>, JsonError> {
    let clients: Vec<ClientData> = serde_json::from_str(json)?;

    let summaries = clients
        .iter()
        .map(|client| summarize(&client.workspace))
        .collect();

    Ok(summaries)
}

fn numbered(id: WorkspaceId) -> Summary {
    (
        Some(id),
        WorkspaceAddress::new(id.to_string()),
        WorkspaceKind::Normal,
    )
}

fn named(address: &str) -> Summary {
    (None, WorkspaceAddress::from(address), WorkspaceKind::Normal)
}

fn special(address: &str) -> Summary {
    (
        None,
        WorkspaceAddress::from(address),
        WorkspaceKind::Special,
    )
}

#[test]
fn monitors_0_57_parse_numbered_named_and_special_workspaces() -> Result<(), JsonError> {
    let parsed = monitor_workspaces(MONITORS_0_57)?;

    assert_eq!(
        parsed,
        vec![
            (numbered(1), Some(special("special:fixture"))),
            (named("fixture"), None),
        ]
    );
    Ok(())
}

#[test]
fn monitors_0_56_match_0_57() -> Result<(), JsonError> {
    assert_eq!(
        monitor_workspaces(MONITORS_0_56)?,
        monitor_workspaces(MONITORS_0_57)?
    );
    Ok(())
}

#[test]
fn workspaces_0_57_parse_every_kind() -> Result<(), JsonError> {
    let parsed = workspace_list(WORKSPACES_0_57)?;

    assert_eq!(
        parsed,
        vec![
            Some(numbered(1)),
            Some(special("special:fixture")),
            Some(named("fixture")),
        ]
    );
    Ok(())
}

#[test]
fn workspaces_0_56_match_0_57() -> Result<(), JsonError> {
    assert_eq!(
        workspace_list(WORKSPACES_0_56)?,
        workspace_list(WORKSPACES_0_57)?
    );
    Ok(())
}

#[test]
fn clients_0_57_parse_window_on_special_workspace() -> Result<(), JsonError> {
    let parsed = client_workspaces(CLIENTS_0_57)?;

    assert_eq!(parsed, vec![special("special:fixture")]);
    Ok(())
}

#[test]
fn clients_0_56_match_0_57() -> Result<(), JsonError> {
    assert_eq!(
        client_workspaces(CLIENTS_0_56)?,
        client_workspaces(CLIENTS_0_57)?
    );
    Ok(())
}
