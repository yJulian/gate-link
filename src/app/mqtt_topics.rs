pub(crate) const NODE_ID: &str = "mqtt_gate";

pub(crate) const COVER_CONFIG_TOPIC: &str = "homeassistant/cover/mqtt_gate/config";
pub(crate) const COVER_COMMAND_TOPIC: &str = "mqtt_gate/cover/set";
pub(crate) const COVER_STATE_TOPIC: &str = "mqtt_gate/cover/state";

pub(crate) const CONTACT_CONFIG_TOPIC: &str = "homeassistant/binary_sensor/mqtt_gate_contact/config";
pub(crate) const CONTACT_STATE_TOPIC: &str = "mqtt_gate/contact/state";

pub(crate) const BUTTON_CONFIG_TOPIC: &str = "homeassistant/button/mqtt_gate_impulse/config";
pub(crate) const BUTTON_COMMAND_TOPIC: &str = "mqtt_gate/impulse/set";

pub(crate) const OPEN_STATE: &str = "open";
pub(crate) const CLOSED_STATE: &str = "closed";
pub(crate) const OPENING_STATE: &str = "opening";
pub(crate) const CLOSING_STATE: &str = "closing";

pub(crate) const OPEN_PAYLOAD: &str = "OPEN";
pub(crate) const CLOSE_PAYLOAD: &str = "CLOSE";
pub(crate) const STOP_PAYLOAD: &str = "STOP";