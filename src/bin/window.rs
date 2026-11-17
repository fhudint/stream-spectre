//! Stream Spectre in a window of its own: the same program as `stream-spectre`,
//! with the dashboard shown in a small web view instead of a browser tab. The
//! overlay stays a browser source in OBS.
//!
//! Closing the window stops the program, after the settings and the request
//! history have been written. Build it with `--features window`.

use std::net::SocketAddr;
use std::process::ExitCode;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Context, anyhow};
use stream_spectre::cli::{self, Command};
use stream_spectre::config::{self, LoadOptions};
use stream_spectre::{APP_NAME, platform, run, server};
use tao::dpi::LogicalSize;
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::window::WindowBuilder;
use tracing_subscriber::EnvFilter;
use wry::WebViewBuilder;

/// How long to wait for the server to start listening.
const START_TIMEOUT: Duration = Duration::from_secs(10);

/// What the server thread tells the window when it is done.
enum Ended {
    Server(Result<(), String>),
}

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    match start() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            tracing::error!("{err:#}");
            ExitCode::FAILURE
        }
    }
}

fn start() -> anyhow::Result<()> {
    let config_path = match cli::parse(std::env::args_os().skip(1))? {
        Command::Help => {
            print!("{}", cli::HELP);
            return Ok(());
        }
        Command::Version => {
            println!("{APP_NAME} {}", stream_spectre::APP_VERSION);
            return Ok(());
        }
        Command::Run { config } => config,
    };
    let loaded = config::load(LoadOptions {
        explicit_path: config_path,
    })?;
    tracing::info!(path = %loaded.path.display(), "config loaded");

    let address = SocketAddr::new(loaded.config.server.bind, loaded.config.server.port);
    let host = server::browser_host(address);
    // A token is only set when the server is open beyond this computer.
    let token = loaded
        .secrets
        .dashboard_token
        .as_ref()
        .map(|token| format!("#token={}", token.expose()))
        .unwrap_or_default();
    let dashboard = format!("http://{host}/dashboard/{token}");
    let origin = format!("http://{host}/");

    // No application ID: with one, a second copy would crash GTK instead of
    // reaching the message that the port is taken. The desktop matches the
    // window to `stream-spectre-window.desktop` by the program's name.
    let event_loop = EventLoopBuilder::<Ended>::with_user_event().build();
    let proxy = event_loop.create_proxy();

    // The program itself runs on a thread of its own; the window only shows it.
    let (stop_server, stop_requested) = tokio::sync::oneshot::channel::<()>();
    let (listening, listening_signal) = std::sync::mpsc::channel::<()>();
    let server_thread = std::thread::spawn(move || {
        let result = run_server(loaded, stop_requested, listening);
        let _ = proxy.send_event(Ended::Server(result.map_err(|err| format!("{err:#}"))));
    });

    let startup = wait_until_listening(address, &listening_signal, &server_thread);
    if let Err(message) = &startup {
        tracing::error!("{message}");
    }

    let window = WindowBuilder::new()
        .with_title("Stream Spectre")
        .with_inner_size(LogicalSize::new(1100.0, 760.0))
        .build(&event_loop)
        .context("cannot open the window")?;

    let builder = match &startup {
        Ok(()) => WebViewBuilder::new().with_url(&dashboard),
        Err(message) => WebViewBuilder::new().with_html(error_page(message)),
    }
    // The window shows the dashboard and nothing else: other addresses and
    // new windows are refused.
    .with_navigation_handler(move |url| {
        url.starts_with(&origin) || url.starts_with("data:") || url.starts_with("about:")
    })
    .with_new_window_req_handler(|_, _| wry::NewWindowResponse::Deny);

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    let _webview = builder.build(&window).context("cannot open the web view")?;
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let _webview = {
        use tao::platform::unix::WindowExtUnix;
        use wry::WebViewBuilderExtUnix;
        let vbox = window
            .default_vbox()
            .ok_or_else(|| anyhow!("the window has no container for the web view"))?;
        builder
            .build_gtk(vbox)
            .context("cannot open the web view")?
    };

    // A window that only says why nothing started stays until it is closed.
    let failed = startup.is_err();
    let mut stop_server = Some(stop_server);
    let mut server_thread = Some(server_thread);
    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                // Gone from the screen at once; the program saves, then ends.
                window.set_visible(false);
                if failed {
                    *control_flow = ControlFlow::Exit;
                    return;
                }
                match stop_server.take() {
                    Some(stop) => {
                        let _ = stop.send(());
                    }
                    None => *control_flow = ControlFlow::Exit,
                }
            }
            Event::UserEvent(Ended::Server(result)) => {
                if let Err(message) = result {
                    tracing::error!("{message}");
                }
                if let Some(thread) = server_thread.take() {
                    let _ = thread.join();
                }
                if !failed {
                    *control_flow = ControlFlow::Exit;
                }
            }
            _ => {}
        }
    })
}

/// Runs the program until `stop_requested` fires (the window was closed) or the
/// system asks it to stop (Ctrl+C in the terminal that started it).
fn run_server(
    loaded: config::Loaded,
    stop_requested: tokio::sync::oneshot::Receiver<()>,
    listening: std::sync::mpsc::Sender<()>,
) -> anyhow::Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("cannot start the async runtime")?;
    runtime.block_on(run::serve(
        loaded,
        async move {
            tokio::select! {
                _ = stop_requested => {}
                () = platform::shutdown_signal() => {}
            }
        },
        move || {
            let _ = listening.send(());
        },
    ))
}

/// Waits until the server says it is listening, or says why it will not.
fn wait_until_listening(
    address: SocketAddr,
    listening: &std::sync::mpsc::Receiver<()>,
    server: &JoinHandle<()>,
) -> Result<(), String> {
    let deadline = Instant::now() + START_TIMEOUT;
    loop {
        if listening.recv_timeout(Duration::from_millis(50)).is_ok() {
            return Ok(());
        }
        if server.is_finished() {
            return Err(format!(
                "The program could not start on {address}. Is another copy already running? See the log for the reason."
            ));
        }
        if Instant::now() >= deadline {
            return Err(format!("Nothing started listening on {address}."));
        }
    }
}

/// A page that says what went wrong, for a window that cannot show the
/// dashboard.
fn error_page(message: &str) -> String {
    let escaped = message
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    format!(
        "<!doctype html><meta charset=\"utf-8\"><title>Stream Spectre</title>\
         <body style=\"font:16px system-ui;background:#0c0d0f;color:#fff;padding:2em\">\
         <h1 style=\"font-size:20px\">Stream Spectre could not start</h1><p>{escaped}</p>"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn address() -> SocketAddr {
        ([127, 0, 0, 1], 8787).into()
    }

    #[test]
    fn the_error_page_shows_the_message_as_text() {
        let page = error_page("a <b>&</b> c");
        assert!(page.contains("a &lt;b&gt;&amp;&lt;/b&gt; c"));
        assert!(!page.contains("<b>"));
    }

    #[test]
    fn a_server_that_ends_before_it_listens_is_reported_with_its_address() {
        let (_listening, signal) = std::sync::mpsc::channel::<()>();
        let ended = std::thread::spawn(|| {});
        while !ended.is_finished() {
            std::thread::sleep(Duration::from_millis(5));
        }
        let message = wait_until_listening(address(), &signal, &ended).unwrap_err();
        assert!(message.contains("127.0.0.1:8787"), "{message}");
    }

    #[test]
    fn a_server_that_says_it_listens_is_not_waited_for_any_longer() {
        let (listening, signal) = std::sync::mpsc::channel::<()>();
        let running = std::thread::spawn(|| std::thread::sleep(Duration::from_millis(300)));
        listening.send(()).unwrap();
        assert_eq!(wait_until_listening(address(), &signal, &running), Ok(()));
    }
}
