//! Relm4/GTK4 backend. Selected at runtime via the `--relm` flag.

mod app;
mod bar;
mod style;
mod theme;

pub mod config;
pub mod hub;
pub mod widgets;

pub fn run() {
    use env_logger::Env;
    let env = Env::new().filter("RS_BAR_LOG").write_style("RS_BAR");
    let _ = env_logger::try_init_from_env(env);

    crate::relm4_bar::config::init();

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("failed to build tokio runtime");
    let _guard = rt.enter();

    // Pass an empty argv to GApplication: we consume our own `--config` /
    // `--relm` flags ourselves, and GTK4's GApplication would otherwise reject
    // them and exit on startup.
    let app = relm4::RelmApp::new("dev.rs-bar.relm4").with_args(vec![]);
    app.run::<app::App>(());
}
