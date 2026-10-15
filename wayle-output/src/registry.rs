use wayle_core::monitor::MonitorInfo;

use crate::OutputEvent;

/// `wl_output` fields as received from the compositor.
#[derive(Debug, Clone, Default)]
pub(crate) struct WlOutput {
    pub name: String,
    pub make: String,
    pub model: String,
    pub description: String,
}

/// `zwlr_output_head_v1` fields. Heads also exist for disabled outputs.
#[derive(Debug, Clone, Default)]
pub(crate) struct Head {
    pub name: Option<String>,
    pub make: Option<String>,
    pub model: Option<String>,
    pub serial: Option<String>,
}

/// Builds one [`MonitorInfo`] per `wl_output`, preferring head data when a
/// head with the same name exists.
pub(crate) fn merge<'a>(
    outputs: &[WlOutput],
    heads: impl IntoIterator<Item = &'a Head> + Clone,
) -> Vec<MonitorInfo> {
    outputs
        .iter()
        .map(|output| {
            let base = MonitorInfo::from_description(
                output.name.as_str(),
                &output.make,
                &output.model,
                &output.description,
            );

            let Some(head) = heads
                .clone()
                .into_iter()
                .find(|head| head.name.as_deref() == Some(output.name.as_str()))
            else {
                return base;
            };

            let make = head.make.as_deref().unwrap_or(&base.make);
            let model = head.model.as_deref().unwrap_or(&base.model);
            let mut info = MonitorInfo::new(base.connector, make, model, head.serial.as_deref());
            // v1 heads and some compositors leave the serial empty
            if info.serial.is_none() {
                info.serial = base.serial;
            }
            info
        })
        .collect()
}

/// Events that turn `old` into `new`, keyed by connector.
pub(crate) fn diff(old: &[MonitorInfo], new: &[MonitorInfo]) -> Vec<OutputEvent> {
    let mut events: Vec<OutputEvent> = old
        .iter()
        .filter(|prev| !new.iter().any(|next| next.connector == prev.connector))
        .map(|prev| OutputEvent::Removed(prev.connector.clone()))
        .collect();

    for next in new {
        match old.iter().find(|prev| prev.connector == next.connector) {
            None => events.push(OutputEvent::Added(next.clone())),
            Some(prev) if prev != next => events.push(OutputEvent::Changed(next.clone())),
            Some(_) => {}
        }
    }

    events
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(name: &str, description: &str) -> WlOutput {
        WlOutput {
            name: name.into(),
            make: "Dell Inc.".into(),
            model: "U2720Q".into(),
            description: description.into(),
        }
    }

    fn head(name: &str, serial: &str) -> Head {
        Head {
            name: Some(name.into()),
            make: Some("Dell Inc.".into()),
            model: Some("U2720Q".into()),
            serial: Some(serial.into()),
        }
    }

    #[test]
    fn head_serial_wins_over_description() {
        let merged = merge(
            &[output("DP-1", "Dell Inc. U2720Q OLD")],
            &[head("DP-1", "NEW")],
        );
        assert_eq!(merged[0].serial.as_deref(), Some("NEW"));
    }

    #[test]
    fn falls_back_to_description_without_head() {
        let merged = merge(
            &[output("DP-1", "Dell Inc. U2720Q ABC (DP-1)")],
            &[head("DP-2", "OTHER")],
        );
        assert_eq!(merged[0].serial.as_deref(), Some("ABC"));
    }

    #[test]
    fn falls_back_to_description_when_head_serial_is_placeholder() {
        let merged = merge(
            &[output("DP-1", "Dell Inc. U2720Q ABC")],
            &[head("DP-1", "0")],
        );
        assert_eq!(merged[0].serial.as_deref(), Some("ABC"));
    }

    #[test]
    fn disabled_heads_are_ignored() {
        let merged = merge(
            &[output("DP-1", "")],
            &[head("eDP-1", "X"), head("DP-1", "Y")],
        );
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].serial.as_deref(), Some("Y"));
    }

    #[test]
    fn diff_reports_add_change_remove() {
        let a = MonitorInfo::new("DP-1", "Dell Inc.", "U2720Q", None);
        let b = MonitorInfo::new("DP-2", "Dell Inc.", "U2720Q", None);
        let b2 = MonitorInfo::new("DP-2", "Dell Inc.", "U2720Q", Some("SN"));
        let c = MonitorInfo::new("DP-3", "Dell Inc.", "U2720Q", None);

        let events = diff(&[a, b], &[b2.clone(), c.clone()]);

        assert_eq!(
            events,
            vec![
                OutputEvent::Removed("DP-1".into()),
                OutputEvent::Changed(b2),
                OutputEvent::Added(c),
            ]
        );
    }

    #[test]
    fn diff_detects_connector_reuse_by_other_monitor() {
        let old = MonitorInfo::new("DP-8", "Ancor", "BE24A", Some("A"));
        let new = MonitorInfo::new("DP-8", "Samsung", "SyncMaster", Some("B"));

        assert_eq!(
            diff(&[old], std::slice::from_ref(&new)),
            vec![OutputEvent::Changed(new)]
        );
    }
}
