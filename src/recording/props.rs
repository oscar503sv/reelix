//! Panic-free property setters.
//!
//! `set_property` / `set_property_from_str` panic when a property is missing
//! or a value is invalid. Encoder capabilities vary with the driver (e.g.
//! Intel's low-power H.264 encoder only offers constant-QP rate control), so
//! encoder tuning goes through these helpers: an unsupported setting is
//! logged and skipped instead of crashing the recording.

use gst::glib;
use gst::prelude::*;

/// Sets a typed property if it exists with that exact type.
pub fn try_set<V: ToValue + glib::types::StaticType>(
    element: &gst::Element,
    name: &str,
    value: V,
) -> bool {
    match element.find_property(name) {
        Some(pspec) if pspec.value_type() == V::static_type() => {
            element.set_property_from_value(name, &value.to_value());
            true
        }
        Some(pspec) => {
            tracing::warn!(
                element = %element.name(),
                property = name,
                expected = %pspec.value_type(),
                "property has a different type; skipped"
            );
            false
        }
        None => {
            tracing::debug!(element = %element.name(), property = name, "property not available");
            false
        }
    }
}

/// Sets an enum property by nick (e.g. `rate-control = "vbr"`) if the value
/// is supported by this element.
pub fn try_set_enum(element: &gst::Element, name: &str, nick: &str) -> bool {
    let Some(pspec) = element.find_property(name) else {
        tracing::debug!(element = %element.name(), property = name, "property not available");
        return false;
    };
    let Some(class) = glib::EnumClass::with_type(pspec.value_type()) else {
        tracing::warn!(element = %element.name(), property = name, "not an enum property");
        return false;
    };
    match class.to_value_by_nick(nick) {
        Some(value) => {
            element.set_property_from_value(name, &value);
            true
        }
        None => {
            tracing::debug!(element = %element.name(), property = name, value = nick, "value not supported");
            false
        }
    }
}

/// Whether an enum property accepts `nick`.
pub fn supports_enum(element: &gst::Element, name: &str, nick: &str) -> bool {
    element
        .find_property(name)
        .and_then(|pspec| glib::EnumClass::with_type(pspec.value_type()))
        .is_some_and(|class| class.value_by_nick(nick).is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn element(factory: &str) -> Option<gst::Element> {
        gst::init().ok()?;
        gst::ElementFactory::make(factory).build().ok()
    }

    #[test]
    fn invalid_values_are_rejected_without_panicking() {
        let Some(queue) = element("queue") else {
            return;
        };
        assert!(try_set_enum(&queue, "leaky", "downstream"));
        assert!(!try_set_enum(&queue, "leaky", "sideways"));
        assert!(!try_set_enum(&queue, "no-such-property", "x"));
        assert!(
            !try_set_enum(&queue, "max-size-buffers", "x"),
            "not an enum"
        );
        assert!(try_set(&queue, "max-size-buffers", 7u32));
        assert_eq!(queue.property::<u32>("max-size-buffers"), 7);
        assert!(!try_set(&queue, "max-size-buffers", 7i64), "wrong type");
        assert!(!try_set(&queue, "no-such-property", 1u32));
        assert!(supports_enum(&queue, "leaky", "upstream"));
        assert!(!supports_enum(&queue, "leaky", "sideways"));
    }
}
