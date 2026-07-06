//! The provisioning config form: served over plain HTTP (no captive-portal DNS
//! hijacking) once the device is in access-point mode. Submitting the form signals
//! `CONFIG_SUBMITTED`, which `main()`'s `run_provisioning_mode` waits on to persist
//! the config and reboot into station mode.

use alloc::string::String;
use core::fmt::Write as _;

use embassy_net::Stack;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use picoserve::extract::Form;
use picoserve::response::Content;
use picoserve::routing::get;
use picoserve::{AppBuilder, AppRouter, Router};

use crate::infra::config::{AppConfig, SubmittedForm};

/// Set once a valid form submission has been persisted; `main()` waits on this to
/// know when to reboot into station mode.
pub static CONFIG_SUBMITTED: Signal<CriticalSectionRawMutex, AppConfig> = Signal::new();

/// Wraps a string body so it's served with an HTML content-type (picoserve has no
/// built-in `Html` wrapper).
struct Html<T>(T);

impl<T: AsRef<str>> Content for Html<T> {
    fn content_type(&self) -> &'static str {
        "text/html; charset=utf-8"
    }

    fn content_length(&self) -> usize {
        self.0.as_ref().len()
    }

    async fn write_content<W: picoserve::io::Write>(self, mut writer: W) -> Result<(), W::Error> {
        writer.write_all(self.0.as_ref().as_bytes()).await
    }
}

fn render_form(error: Option<&str>) -> String {
    let mut page = String::new();
    let _ = write!(
        page,
        "<!doctype html><html><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <title>mqtt_gate setup</title></head><body>\
         <h1>mqtt_gate setup</h1>"
    );
    if let Some(error) = error {
        let _ = write!(page, "<p style=\"color:red\">{error}</p>");
    }
    let _ = write!(
        page,
        "<form method=\"post\" action=\"/\">\
         <label>Wi-Fi SSID<br><input name=\"ssid\" required maxlength=\"32\"></label><br>\
         <label>Wi-Fi password<br><input name=\"wifi_password\" type=\"password\" maxlength=\"64\"></label><br>\
         <label>MQTT host<br><input name=\"mqtt_host\" required maxlength=\"128\"></label><br>\
         <label>MQTT port<br><input name=\"mqtt_port\" type=\"number\" value=\"1883\" required></label><br>\
         <label>MQTT username (optional)<br><input name=\"mqtt_username\" maxlength=\"64\"></label><br>\
         <label>MQTT password (optional)<br><input name=\"mqtt_password\" type=\"password\" maxlength=\"64\"></label><br>\
         <button type=\"submit\">Save &amp; reboot</button>\
         </form></body></html>"
    );
    page
}

async fn index() -> Html<String> {
    Html(render_form(None))
}

async fn submit(Form(form): Form<SubmittedForm>) -> Html<String> {
    match form.into_config() {
        Ok(cfg) => {
            CONFIG_SUBMITTED.signal(cfg);
            Html(String::from(
                "<!doctype html><html><body><p>Saved. Rebooting into station mode...</p></body></html>",
            ))
        }
        Err(error) => Html(render_form(Some(error))),
    }
}

pub struct AppProps;

impl AppBuilder for AppProps {
    type PathRouter = impl picoserve::routing::PathRouter;

    fn build_app(self) -> Router<Self::PathRouter> {
        Router::new().route("/", get(index).post(submit))
    }
}

/// One worker instance handling connections to the provisioning form. A single
/// worker is enough for a setup wizard used by one person at a time, and it keeps
/// the static RAM footprint down (each worker's buffers are part of the fixed-size
/// embassy task pool, not the heap).
#[embassy_executor::task(pool_size = 1)]
pub async fn task(
    id: usize,
    stack: Stack<'static>,
    app: &'static AppRouter<AppProps>,
    config: &'static picoserve::Config,
) {
    let mut tcp_rx_buffer = [0u8; 512];
    let mut tcp_tx_buffer = [0u8; 512];
    let mut http_buffer = [0u8; 1024];

    picoserve::Server::new(app, config, &mut http_buffer)
        .listen_and_serve(id, stack, 80, &mut tcp_rx_buffer, &mut tcp_tx_buffer)
        .await;
}
