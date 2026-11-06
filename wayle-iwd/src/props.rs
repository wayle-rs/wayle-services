//! Decoding IWD property values into model fields.
//!
//! IWD delivers properties as `{name: variant}` maps in three places:
//! `GetManagedObjects`, `InterfacesAdded` and `PropertiesChanged`. Models route
//! all three through the same per-interface `apply` function, built from these
//! helpers, so each property is decoded in exactly one place.

use std::collections::HashMap;

use tracing::debug;
use zbus::zvariant::OwnedValue;

/// Properties of one D-Bus interface, keyed by property name.
pub(crate) type PropertyMap = HashMap<String, OwnedValue>;

/// Decodes `value` as `T`, logging (and returning `None`) on a type mismatch.
fn decode<T>(name: &str, value: OwnedValue) -> Option<T>
where
    T: TryFrom<OwnedValue>,
{
    match T::try_from(value) {
        Ok(decoded) => Some(decoded),
        Err(_) => {
            debug!(property = name, "cannot decode iwd property");
            None
        }
    }
}

/// Sets `field` to the decoded `value`.
pub(crate) fn assign<T>(field: &mut T, name: &str, value: OwnedValue)
where
    T: TryFrom<OwnedValue>,
{
    if let Some(decoded) = decode(name, value) {
        *field = decoded;
    }
}

/// Sets an optional `field` to `Some(decoded value)`.
pub(crate) fn assign_some<T>(field: &mut Option<T>, name: &str, value: OwnedValue)
where
    T: TryFrom<OwnedValue>,
{
    if let Some(decoded) = decode(name, value) {
        *field = Some(decoded);
    }
}
