//! Monitor identity and the selector syntax used to target monitors in config.
//!
//! | Selector        | Matches                                         |
//! |-----------------|-------------------------------------------------|
//! | `*`             | any monitor                                     |
//! | `DP-1`          | connector name, exact                           |
//! | `desc:<text>`   | prefix of `"<make> <model> <serial>"`           |
//! | `sn:<serial>`   | serial number, exact                            |
//!
//! When several selectors match one monitor, the most specific wins:
//! `sn:` > `desc:` (longer prefix first) > connector > `*`.

use std::{fmt, str::FromStr};

/// Identity of a connected monitor.
///
/// Fields are whitespace-normalized. Placeholder serials reported by
/// monitors without a real one (`0`, `0x00000000`, `Unknown`) become `None`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct MonitorInfo {
    /// Connector name, e.g. `DP-1`. Not stable across hotplug on docks and MST hubs.
    pub connector: String,
    /// Manufacturer as reported by the compositor.
    pub make: String,
    /// Model as reported by the compositor.
    pub model: String,
    /// Serial number, if the monitor reports a real one.
    pub serial: Option<String>,
}

impl MonitorInfo {
    /// Creates an identity from separate fields.
    pub fn new(
        connector: impl Into<String>,
        make: &str,
        model: &str,
        serial: Option<&str>,
    ) -> Self {
        Self {
            connector: connector.into(),
            make: normalize(make),
            model: normalize(model),
            serial: serial.map(normalize).filter(|s| !is_placeholder_serial(s)),
        }
    }

    /// Creates an identity, taking the serial from a `wl_output` description.
    ///
    /// Compositors put the serial after make and model, e.g.
    /// `"Dell Inc. U2720Q ABC123"` (Hyprland) or
    /// `"Dell Inc. U2720Q ABC123 (DP-1)"` (wlroots).
    pub fn from_description(
        connector: impl Into<String>,
        make: &str,
        model: &str,
        description: &str,
    ) -> Self {
        let mut info = Self::new(connector, make, model, None);
        info.serial = serial_from_description(&info.make, &info.model, description);
        info
    }

    /// `"<make> <model> <serial>"` with empty parts left out.
    pub fn description(&self) -> String {
        [
            self.make.as_str(),
            self.model.as_str(),
            self.serial.as_deref().unwrap_or_default(),
        ]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
    }
}

/// Parsed monitor selector. See the module docs for the syntax.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum MonitorSelector {
    /// `*`
    Any,
    /// Bare connector name, e.g. `DP-1`.
    Connector(String),
    /// `desc:<prefix>`
    Description(String),
    /// `sn:<serial>`
    Serial(String),
}

/// Specificity of a selector; higher wins. Compare with `Ord`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Specificity(u8, usize);

impl MonitorSelector {
    /// Whether this selector matches `monitor`.
    pub fn matches(&self, monitor: &MonitorInfo) -> bool {
        match self {
            Self::Any => true,
            Self::Connector(name) => monitor.connector == *name,
            Self::Description(prefix) => monitor.description().starts_with(prefix.as_str()),
            Self::Serial(serial) => monitor.serial.as_deref() == Some(serial.as_str()),
        }
    }

    /// Priority among selectors that match the same monitor.
    pub fn specificity(&self) -> Specificity {
        match self {
            Self::Any => Specificity(0, 0),
            Self::Connector(_) => Specificity(1, 0),
            Self::Description(prefix) => Specificity(2, prefix.len()),
            Self::Serial(_) => Specificity(3, 0),
        }
    }
}

impl FromStr for MonitorSelector {
    type Err = SelectorError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let input = input.trim();

        if input == "*" {
            return Ok(Self::Any);
        }
        if let Some(value) = input.strip_prefix("desc:") {
            return non_empty(normalize(value), input).map(Self::Description);
        }
        if let Some(value) = input.strip_prefix("sn:") {
            return non_empty(value.trim().to_owned(), input).map(Self::Serial);
        }
        // connector names never contain ':'
        if let Some((prefix, _)) = input.split_once(':') {
            return Err(SelectorError::UnknownPrefix(prefix.to_owned()));
        }

        non_empty(input.to_owned(), input).map(Self::Connector)
    }
}

impl fmt::Display for MonitorSelector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Any => f.write_str("*"),
            Self::Connector(name) => f.write_str(name),
            Self::Description(prefix) => write!(f, "desc:{prefix}"),
            Self::Serial(serial) => write!(f, "sn:{serial}"),
        }
    }
}

/// Invalid monitor selector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectorError {
    /// Selector or its value is empty.
    Empty(String),
    /// Prefix other than `desc:` or `sn:`.
    UnknownPrefix(String),
}

impl fmt::Display for SelectorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty(input) if input.is_empty() => f.write_str("empty monitor selector"),
            Self::Empty(input) => write!(f, "monitor selector '{input}' has no value"),
            Self::UnknownPrefix(prefix) => write!(
                f,
                "unknown monitor selector prefix '{prefix}:' (expected 'desc:' or 'sn:')"
            ),
        }
    }
}

impl std::error::Error for SelectorError {}

/// Returns the item whose selector best matches `monitor`.
///
/// Items with invalid selectors are skipped. On equal specificity the
/// first item wins.
pub fn best_match<'a, T>(
    items: &'a [T],
    selector: impl Fn(&T) -> &str,
    monitor: &MonitorInfo,
) -> Option<&'a T> {
    let mut best: Option<(Specificity, &T)> = None;

    for item in items {
        let Ok(parsed) = selector(item).parse::<MonitorSelector>() else {
            continue;
        };
        if !parsed.matches(monitor) {
            continue;
        }
        let specificity = parsed.specificity();
        if best.is_none_or(|(current, _)| specificity > current) {
            best = Some((specificity, item));
        }
    }

    best.map(|(_, item)| item)
}

fn normalize(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn non_empty(value: String, input: &str) -> Result<String, SelectorError> {
    if value.is_empty() {
        Err(SelectorError::Empty(input.to_owned()))
    } else {
        Ok(value)
    }
}

fn is_placeholder_serial(serial: &str) -> bool {
    let digits = serial
        .strip_prefix("0x")
        .or_else(|| serial.strip_prefix("0X"))
        .unwrap_or(serial);

    serial.is_empty() || serial.eq_ignore_ascii_case("unknown") || digits.chars().all(|c| c == '0')
}

fn serial_from_description(make: &str, model: &str, description: &str) -> Option<String> {
    let mut description = normalize(description);

    // wlroots appends " (<connector>)" or " (<connector> via <adapter>)"
    if description.ends_with(')')
        && let Some(start) = description.rfind(" (")
    {
        description.truncate(start);
    }

    if make.is_empty() || model.is_empty() {
        return None;
    }

    let head = format!("{make} {model} ");
    description
        .strip_prefix(&head)
        .map(str::to_owned)
        .filter(|serial| !is_placeholder_serial(serial))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ancor(connector: &str) -> MonitorInfo {
        MonitorInfo::new(
            connector,
            "Ancor Communications Inc",
            "BE24A",
            Some("XYZ789"),
        )
    }

    fn parse(input: &str) -> Result<MonitorSelector, SelectorError> {
        input.parse()
    }

    fn selects(input: &str, monitor: &MonitorInfo) -> bool {
        parse(input).is_ok_and(|selector| selector.matches(monitor))
    }

    #[test]
    fn parses_all_forms() {
        assert_eq!(parse("*"), Ok(MonitorSelector::Any));
        assert_eq!(
            parse(" DP-1 "),
            Ok(MonitorSelector::Connector("DP-1".into()))
        );
        assert_eq!(
            parse("desc:Dell  Inc.   U2720Q "),
            Ok(MonitorSelector::Description("Dell Inc. U2720Q".into()))
        );
        assert_eq!(
            parse("sn: ABC123 "),
            Ok(MonitorSelector::Serial("ABC123".into()))
        );
    }

    #[test]
    fn rejects_invalid() {
        assert!(matches!(
            "".parse::<MonitorSelector>(),
            Err(SelectorError::Empty(_))
        ));
        assert!(matches!(
            "desc: ".parse::<MonitorSelector>(),
            Err(SelectorError::Empty(_))
        ));
        assert!(matches!(
            "sn:".parse::<MonitorSelector>(),
            Err(SelectorError::Empty(_))
        ));
        assert_eq!(
            "make:Dell".parse::<MonitorSelector>(),
            Err(SelectorError::UnknownPrefix("make".into()))
        );
    }

    #[test]
    fn display_round_trips() {
        for input in ["*", "DP-1", "desc:Dell Inc. U2720Q", "sn:ABC123"] {
            assert_eq!(parse(input).map(|s| s.to_string()), Ok(input.to_owned()));
        }
    }

    #[test]
    fn matches_each_kind() {
        let monitor = ancor("DP-8");

        assert!(selects("*", &monitor));
        assert!(selects("DP-8", &monitor));
        assert!(!selects("DP-6", &monitor));
        assert!(selects(
            "desc:Ancor Communications Inc BE24A XYZ789",
            &monitor
        ));
        assert!(selects("desc:Ancor Communications Inc BE24A", &monitor));
        assert!(!selects("desc:Samsung", &monitor));
        assert!(selects("sn:XYZ789", &monitor));
        assert!(!selects("sn:H8LMQS1145", &monitor));
    }

    #[test]
    fn placeholder_serials_are_dropped() {
        for serial in ["", "0", "0x00000000", "Unknown", "  "] {
            let monitor = MonitorInfo::new("DP-1", "Dell Inc.", "U2720Q", Some(serial));
            assert_eq!(monitor.serial, None, "{serial:?}");
            assert_eq!(monitor.description(), "Dell Inc. U2720Q");
        }
    }

    #[test]
    fn serial_from_hyprland_description() {
        let monitor = MonitorInfo::from_description(
            "DP-8",
            "Ancor Communications Inc",
            "BE24A",
            "Ancor Communications Inc BE24A XYZ789",
        );
        assert_eq!(monitor, ancor("DP-8"));
    }

    #[test]
    fn serial_from_wlroots_description() {
        let monitor = MonitorInfo::from_description(
            "DP-1",
            "Dell Inc.",
            "U2720Q",
            "Dell Inc. U2720Q ABC123 (DP-1 via HDMI)",
        );
        assert_eq!(monitor.serial.as_deref(), Some("ABC123"));
    }

    #[test]
    fn no_serial_when_description_lacks_one() {
        for description in [
            "Dell Inc. U2720Q",
            "Dell Inc. U2720Q (DP-1)",
            "",
            "headless",
        ] {
            let monitor = MonitorInfo::from_description("DP-1", "Dell Inc.", "U2720Q", description);
            assert_eq!(monitor.serial, None, "{description:?}");
        }
    }

    #[test]
    fn headless_output_matches_by_connector_only() {
        let monitor = MonitorInfo::from_description("HEADLESS-1", "", "", "");

        assert!(selects("HEADLESS-1", &monitor));
        assert!(!selects("desc:Dell", &monitor));
        assert!(!selects("sn:0", &monitor));
    }

    #[test]
    fn best_match_prefers_specific_selectors() {
        let entries = [
            "*",
            "DP-8",
            "desc:Ancor",
            "desc:Ancor Communications Inc BE24A",
            "sn:XYZ789",
        ];
        let monitor = ancor("DP-8");

        for end in (1..=entries.len()).rev() {
            let found = best_match(&entries[..end], |entry| entry, &monitor);
            assert_eq!(found, Some(&entries[end - 1]));
        }
    }

    #[test]
    fn best_match_first_wins_on_tie_and_skips_invalid() {
        let entries = ["foo:bar", "desc:Ancor", "desc:Samsu", "desc:Ancor"];
        let monitor = ancor("DP-8");

        let found = best_match(&entries, |entry| entry, &monitor);
        assert!(found.is_some_and(|entry| std::ptr::eq(entry, &entries[1])));
    }

    #[test]
    fn best_match_none_without_wildcard() {
        let entries = ["DP-6", "sn:other"];
        assert!(best_match(&entries, |entry| entry, &ancor("DP-8")).is_none());
    }
}
