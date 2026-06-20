//! Connects to the compositor and dumps the current workspace/group state.

use std::time::Duration;

use wayle_ext_workspace::ExtWorkspaceService;

fn main() -> wayle_ext_workspace::Result<()> {
    let service = ExtWorkspaceService::new()?;

    std::thread::sleep(Duration::from_millis(200));

    for group in service.groups.get().values() {
        println!(
            "group {} outputs={:?} workspaces={:?} caps={:#x}",
            group.key,
            group.outputs.get(),
            group.workspaces.get(),
            group.capabilities.get()
        );
    }

    for workspace in service.workspaces.get().values() {
        println!(
            "workspace {} id={:?} name={:?} coords={:?} active={} urgent={} hidden={}",
            workspace.key,
            workspace.id.get(),
            workspace.name.get(),
            workspace.coordinates.get(),
            workspace.is_active(),
            workspace.is_urgent(),
            workspace.is_hidden()
        );
    }

    Ok(())
}
