#![no_std]
#![feature(impl_trait_in_assoc_type)]

extern crate alloc;

/// Turns a value into a `&'static mut` by leaking it into a `static_cell::StaticCell`.
///
/// Embassy tasks require `'static` references for anything they borrow, so this is the
/// standard way to promote a runtime-constructed value (stack resources, driver instances,
/// etc.) to `'static` without actually using a global `static` item for each one.
#[macro_export]
macro_rules! mk_static {
    ($t:ty,$val:expr) => {{
        static STATIC_CELL: static_cell::StaticCell<$t> = static_cell::StaticCell::new();
        #[deny(unused_attributes)]
        let x = STATIC_CELL.uninit().write($val);
        x
    }};
}

pub mod config;
pub mod dhcp_server;
pub mod mqtt_client;
pub mod provisioning_http;
pub mod reset_button;
pub mod storage;
pub mod wifi_ap;
pub mod wifi_sta;
