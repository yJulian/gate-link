//! The provisioning config form: served over plain HTTP (no captive-portal DNS
//! hijacking) once the device is in access-point mode. Submitting the form signals
//! `CONFIG_SUBMITTED`, which `main()`'s `run_provisioning_mode` waits on to persist
//! the config and reboot into station mode.

use alloc::string::String;

use app_core::config::{AppConfig, SubmittedForm};
use app_core::provisioning::{SAVED_PAGE, form_page};
use embassy_net::Stack;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use picoserve::extract::Form;
use picoserve::response::Content;
use picoserve::routing::get;
use picoserve::{AppBuilder, AppRouter, Router};

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

async fn index() -> Html<String> {
    Html(form_page(None))
}

async fn submit(Form(form): Form<SubmittedForm>) -> Html<String> {
    match form.into_config() {
        Ok(cfg) => {
            CONFIG_SUBMITTED.signal(cfg);
            Html(String::from(SAVED_PAGE))
        }
        Err(error) => Html(form_page(Some(error))),
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
