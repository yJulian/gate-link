pub(crate) const NODE_ID: &str = "mqtt_gate";

pub(crate) const COVER_CONFIG_TOPIC: &str = "homeassistant/cover/mqtt_gate/config";
pub(crate) const COVER_COMMAND_TOPIC: &str = "mqtt_gate/cover/set";
pub(crate) const COVER_STATE_TOPIC: &str = "mqtt_gate/cover/state";

pub(crate) const WIND_CONFIG_TOPIC: &str = "homeassistant/binary_sensor/mqtt_gate_wind/config";
pub(crate) const WIND_STATE_TOPIC: &str = "mqtt_gate/wind/state";

/// Discovery topic of the former spare-input binary sensor (that input now
/// arms the barrier bypass) - cleared on connect so HA drops the entity.
pub(crate) const SPARE_INPUT_CONFIG_TOPIC: &str =
    "homeassistant/binary_sensor/mqtt_gate_spare_input/config";

pub(crate) const BARRIER_BYPASS_CONFIG_TOPIC: &str =
    "homeassistant/switch/mqtt_gate_barrier_bypass/config";
pub(crate) const BARRIER_BYPASS_COMMAND_TOPIC: &str = "mqtt_gate/barrier_bypass/set";
pub(crate) const BARRIER_BYPASS_STATE_TOPIC: &str = "mqtt_gate/barrier_bypass/state";

pub(crate) const BUTTON_CONFIG_TOPIC: &str = "homeassistant/button/mqtt_gate_impulse/config";
pub(crate) const BUTTON_COMMAND_TOPIC: &str = "mqtt_gate/impulse/set";
pub(crate) const RESET_WIND_LOCK_PAYLOAD: &str = "reset_anemometer";

pub(crate) const OPEN_STATE: &str = "open";
pub(crate) const CLOSED_STATE: &str = "closed";
pub(crate) const OPENING_STATE: &str = "opening";
pub(crate) const CLOSING_STATE: &str = "closing";

pub(crate) const OPEN_PAYLOAD: &str = "OPEN";
pub(crate) const CLOSE_PAYLOAD: &str = "CLOSE";
pub(crate) const STOP_PAYLOAD: &str = "STOP";

pub(crate) const ON_PAYLOAD: &str = "ON";
pub(crate) const OFF_PAYLOAD: &str = "OFF";
