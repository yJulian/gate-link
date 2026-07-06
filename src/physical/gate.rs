use log::info;

pub(crate) fn open() {
    info!("opening");
}

pub(crate) fn close() {
    info!("closing");
}

pub(crate) fn stop() {
    info!("stopping");
}

pub(crate) fn toggle() {
    info!("toggling");
}