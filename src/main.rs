const BACKEND: &str = "GPUI";

mod gpui_bar;
mod iced_bar;

fn main() {
    // Suppress zbus/tokio "no reactor running" panics on worker threads.
    // This affects both backends because system-tray's zbus dependency
    // can spawn threads that hit this.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let msg = info.to_string();
        if msg.contains("no reactor running") {
            return;
        }
        default_hook(info);
    }));

    match BACKEND {
        "GPUI" => gpui_bar::run(),
        "ICED" => iced_bar::run(),
        _ => panic!("Unknown backend: {BACKEND}"),
    }
}
