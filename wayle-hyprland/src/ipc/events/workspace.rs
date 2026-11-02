use tokio::sync::broadcast::Sender;

use crate::{Error, HyprlandEvent, Result, WorkspaceId, WorkspaceIdentity};

pub(crate) fn resolve_event_workspace(
    event: &str,
    data: &str,
    field: &str,
    name: &str,
) -> Result<WorkspaceIdentity> {
    WorkspaceIdentity::from_event_field(field, name).ok_or_else(|| Error::EventParseError {
        event_data: format!("{event}>>{data}"),
        field: "workspace",
        expected: "workspace address or id",
        value: field.to_string(),
    })
}

pub(crate) fn handle_workspace(data: &str, hyprland_tx: Sender<HyprlandEvent>) -> Result<()> {
    hyprland_tx.send(HyprlandEvent::Workspace {
        name: data.to_string(),
    })?;

    Ok(())
}

pub(crate) fn handle_workspace_v2(
    event: &str,
    data: &str,
    hyprland_tx: Sender<HyprlandEvent>,
) -> Result<()> {
    let Some((workspace_field, name)) = data.split_once(',') else {
        return Err(Error::EventParseError {
            event_data: format!("{event}>>{data}"),
            field: "workspace_data",
            expected: "comma-separated address,name",
            value: data.to_string(),
        });
    };

    let identity = resolve_event_workspace(event, data, workspace_field, name)?;

    hyprland_tx.send(HyprlandEvent::WorkspaceV2 {
        id: identity.id,
        address: identity.address,
        name: name.to_string(),
    })?;

    Ok(())
}

pub(crate) fn handle_create_workspace(
    data: &str,
    hyprland_tx: Sender<HyprlandEvent>,
) -> Result<()> {
    hyprland_tx.send(HyprlandEvent::CreateWorkspace {
        name: data.to_string(),
    })?;

    Ok(())
}

pub(crate) fn handle_create_workspace_v2(
    event: &str,
    data: &str,
    hyprland_tx: Sender<HyprlandEvent>,
) -> Result<()> {
    let Some((workspace_field, name)) = data.split_once(',') else {
        return Err(Error::EventParseError {
            event_data: format!("{event}>>{data}"),
            field: "workspace_data",
            expected: "comma-separated address,name",
            value: data.to_string(),
        });
    };

    let identity = resolve_event_workspace(event, data, workspace_field, name)?;

    hyprland_tx.send(HyprlandEvent::CreateWorkspaceV2 {
        id: identity.id,
        address: identity.address,
        name: name.to_string(),
    })?;

    Ok(())
}

pub(crate) fn handle_destroy_workspace(
    data: &str,
    hyprland_tx: Sender<HyprlandEvent>,
) -> Result<()> {
    hyprland_tx.send(HyprlandEvent::DestroyWorkspace {
        name: data.to_string(),
    })?;

    Ok(())
}

pub(crate) fn handle_destroy_workspace_v2(
    event: &str,
    data: &str,
    hyprland_tx: Sender<HyprlandEvent>,
) -> Result<()> {
    let Some((workspace_field, name)) = data.split_once(',') else {
        return Err(Error::EventParseError {
            event_data: format!("{event}>>{data}"),
            field: "workspace_data",
            expected: "comma-separated address,name",
            value: data.to_string(),
        });
    };

    let identity = resolve_event_workspace(event, data, workspace_field, name)?;

    hyprland_tx.send(HyprlandEvent::DestroyWorkspaceV2 {
        id: identity.id,
        address: identity.address,
        name: name.to_string(),
    })?;

    Ok(())
}

pub(crate) fn handle_move_workspace(
    event: &str,
    data: &str,
    hyprland_tx: Sender<HyprlandEvent>,
) -> Result<()> {
    let Some((name, monitor)) = data.split_once(',') else {
        return Err(Error::EventParseError {
            event_data: format!("{event}>>{data}"),
            field: "workspace_data",
            expected: "comma-separated name,monitor",
            value: data.to_string(),
        });
    };

    hyprland_tx.send(HyprlandEvent::MoveWorkspace {
        name: name.to_string(),
        monitor: monitor.to_string(),
    })?;

    Ok(())
}

pub(crate) fn handle_move_workspace_v2(
    event: &str,
    data: &str,
    hyprland_tx: Sender<HyprlandEvent>,
) -> Result<()> {
    let parts: Vec<&str> = data.split(',').collect();
    let [workspace_field, name, monitor] = parts.as_slice() else {
        return Err(Error::EventParseError {
            event_data: format!("{event}>>{data}"),
            field: "workspace_data",
            expected: "3 comma-separated values (address,name,monitor)",
            value: data.to_string(),
        });
    };

    let identity = resolve_event_workspace(event, data, workspace_field, name)?;

    hyprland_tx.send(HyprlandEvent::MoveWorkspaceV2 {
        id: identity.id,
        address: identity.address,
        name: (*name).to_string(),
        monitor: (*monitor).to_string(),
    })?;

    Ok(())
}

pub(crate) fn handle_rename_workspace(
    event: &str,
    data: &str,
    hyprland_tx: Sender<HyprlandEvent>,
) -> Result<()> {
    let Some((workspace_field, new_name)) = data.split_once(',') else {
        return Err(Error::EventParseError {
            event_data: format!("{event}>>{data}"),
            field: "workspace_data",
            expected: "comma-separated address,new_name",
            value: data.to_string(),
        });
    };

    let identity = resolve_event_workspace(event, data, workspace_field, new_name)?;

    hyprland_tx.send(HyprlandEvent::RenameWorkspace {
        id: identity.id,
        address: identity.address,
        new_name: new_name.to_string(),
    })?;

    Ok(())
}

pub(crate) fn handle_change_workspace_id(
    event: &str,
    data: &str,
    hyprland_tx: Sender<HyprlandEvent>,
) -> Result<()> {
    let parse_id = |field: &'static str, value: &str| {
        value
            .parse::<WorkspaceId>()
            .map_err(|_| Error::EventParseError {
                event_data: format!("{event}>>{data}"),
                field,
                expected: "integer",
                value: value.to_string(),
            })
    };

    let Some((old_id, new_id)) = data.split_once(',') else {
        return Err(Error::EventParseError {
            event_data: format!("{event}>>{data}"),
            field: "workspace_ids",
            expected: "comma-separated old_id,new_id",
            value: data.to_string(),
        });
    };

    hyprland_tx.send(HyprlandEvent::ChangeWorkspaceId {
        old_id: parse_id("old_id", old_id)?,
        new_id: parse_id("new_id", new_id)?,
    })?;

    Ok(())
}

pub(crate) fn handle_active_special(
    event: &str,
    data: &str,
    hyprland_tx: Sender<HyprlandEvent>,
) -> Result<()> {
    let Some((workspace, monitor)) = data.split_once(',') else {
        return Err(Error::EventParseError {
            event_data: format!("{event}>>{data}"),
            field: "special_workspace_data",
            expected: "comma-separated workspace,monitor",
            value: data.to_string(),
        });
    };

    hyprland_tx.send(HyprlandEvent::ActiveSpecial {
        workspace: workspace.to_string(),
        monitor: monitor.to_string(),
    })?;

    Ok(())
}

pub(crate) fn handle_active_special_v2(
    event: &str,
    data: &str,
    hyprland_tx: Sender<HyprlandEvent>,
) -> Result<()> {
    let parts: Vec<&str> = data.split(',').collect();
    let [workspace_field, workspace, monitor] = parts.as_slice() else {
        return Err(Error::EventParseError {
            event_data: format!("{event}>>{data}"),
            field: "special_workspace_data",
            expected: "3 comma-separated values (address,workspace,monitor)",
            value: data.to_string(),
        });
    };

    let identity = WorkspaceIdentity::from_event_field(workspace_field, workspace);

    hyprland_tx.send(HyprlandEvent::ActiveSpecialV2 {
        address: identity.map(|special| special.address),
        workspace: (*workspace).to_string(),
        monitor: (*monitor).to_string(),
    })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{error::Error as StdError, result::Result as StdResult};

    use tokio::sync::broadcast;

    use super::*;
    use crate::{WorkspaceAddress, ipc::events::monitor::handle_focused_mon_v2};

    type TestResult = StdResult<(), Box<dyn StdError>>;

    fn workspace_v2(data: &str) -> StdResult<HyprlandEvent, Box<dyn StdError>> {
        let (event_tx, mut event_rx) = broadcast::channel(4);
        handle_workspace_v2("workspacev2", data, event_tx)?;

        Ok(event_rx.try_recv()?)
    }

    fn workspace_v2_identity(
        event: &HyprlandEvent,
    ) -> Option<(Option<WorkspaceId>, &WorkspaceAddress)> {
        match event {
            HyprlandEvent::WorkspaceV2 { id, address, .. } => Some((*id, address)),
            _ => None,
        }
    }

    #[test]
    fn workspace_v2_numbered_carries_id_and_address() -> TestResult {
        let event = workspace_v2("5,5")?;

        assert_eq!(
            workspace_v2_identity(&event),
            Some((Some(5), &WorkspaceAddress::from("5")))
        );
        Ok(())
    }

    #[test]
    fn workspace_v2_named_matches_across_versions() -> TestResult {
        let legacy = workspace_v2("-1338,chat")?;
        let current = workspace_v2("chat,chat")?;

        assert_eq!(
            workspace_v2_identity(&legacy),
            Some((None, &WorkspaceAddress::from("chat")))
        );
        assert_eq!(
            workspace_v2_identity(&legacy),
            workspace_v2_identity(&current)
        );
        Ok(())
    }

    #[test]
    fn workspace_v2_rejects_empty_workspace_field() {
        let (event_tx, _event_rx) = broadcast::channel(4);

        let result = handle_workspace_v2("workspacev2", ",", event_tx);

        assert!(matches!(result, Err(Error::EventParseError { .. })));
    }

    #[test]
    fn active_special_v2_closed_has_no_address() -> TestResult {
        let (event_tx, mut event_rx) = broadcast::channel(4);
        handle_active_special_v2("activespecialv2", ",,DP-1", event_tx)?;

        let event = event_rx.try_recv()?;

        assert!(matches!(
            event,
            HyprlandEvent::ActiveSpecialV2 { address: None, ref monitor, .. } if monitor == "DP-1"
        ));
        Ok(())
    }

    #[test]
    fn active_special_v2_open_matches_across_versions() -> TestResult {
        let (event_tx, mut event_rx) = broadcast::channel(4);
        handle_active_special_v2(
            "activespecialv2",
            "-98,special:scratch,DP-1",
            event_tx.clone(),
        )?;
        handle_active_special_v2(
            "activespecialv2",
            "special:scratch,special:scratch,DP-1",
            event_tx,
        )?;

        let expected = Some(WorkspaceAddress::from("special:scratch"));

        for _ in 0..2 {
            let event = event_rx.try_recv()?;
            assert!(matches!(
                event,
                HyprlandEvent::ActiveSpecialV2 { ref address, .. } if *address == expected
            ));
        }
        Ok(())
    }

    #[test]
    fn change_workspace_id_parses_both_numbers() -> TestResult {
        let (event_tx, mut event_rx) = broadcast::channel(4);
        handle_change_workspace_id("changeworkspaceid", "3,7", event_tx)?;

        let event = event_rx.try_recv()?;

        assert!(matches!(
            event,
            HyprlandEvent::ChangeWorkspaceId {
                old_id: 3,
                new_id: 7
            }
        ));
        Ok(())
    }

    #[test]
    fn change_workspace_id_rejects_non_numeric() {
        let (event_tx, _event_rx) = broadcast::channel(4);

        let result = handle_change_workspace_id("changeworkspaceid", "3,chat", event_tx);

        assert!(matches!(
            result,
            Err(Error::EventParseError {
                field: "new_id",
                ..
            })
        ));
    }

    #[test]
    fn focused_mon_v2_legacy_named_workspace_has_no_address() -> TestResult {
        let (event_tx, mut event_rx) = broadcast::channel(4);
        handle_focused_mon_v2("focusedmonv2", "DP-1,-1338", event_tx)?;

        let event = event_rx.try_recv()?;

        assert!(matches!(
            event,
            HyprlandEvent::FocusedMonV2 {
                workspace_id: None,
                workspace_address: None,
                ..
            }
        ));
        Ok(())
    }

    #[test]
    fn focused_mon_v2_named_workspace_has_address() -> TestResult {
        let (event_tx, mut event_rx) = broadcast::channel(4);
        handle_focused_mon_v2("focusedmonv2", "DP-1,chat", event_tx)?;

        let event = event_rx.try_recv()?;
        let expected = Some(WorkspaceAddress::from("chat"));

        assert!(matches!(
            event,
            HyprlandEvent::FocusedMonV2 { workspace_id: None, ref workspace_address, .. }
                if *workspace_address == expected
        ));
        Ok(())
    }
}
