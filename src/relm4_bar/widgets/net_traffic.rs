//! Network-traffic widget. Subscribes to `hub::net_traffic` and renders the
//! aggregate download / upload rate as `↓ <rate>  ↑ <rate>` in one capsule.
//! Each direction's label dims while that direction is idle.

use gtk::prelude::*;
use relm4::prelude::*;

use crate::relm4_bar::hub;
use crate::relm4_bar::hub::net_traffic::NetTrafficSample;

use super::{NamedWidget, WidgetInit, capsule, set_exclusive_class};

/// Dim (idle) / normal color classes, toggled per direction by `update`.
const COLOR_CLASSES: &[&str] = &["net-traffic-norm", "net-traffic-dim"];

pub struct NetTraffic {
    /// Last-displayed label strings, for the coalescing check in `update`.
    down: String,
    up: String,
    /// Held so `update` can rewrite + recolor them.
    down_label: gtk::Label,
    up_label: gtk::Label,
}

#[derive(Debug)]
pub enum NetTrafficMsg {
    Update(NetTrafficSample),
}

#[relm4::component(pub)]
impl SimpleComponent for NetTraffic {
    type Init = WidgetInit;
    type Input = NetTrafficMsg;
    type Output = ();

    view! {
        gtk::Box {
            set_orientation: gtk::Orientation::Horizontal,
            set_spacing: 6,
            set_valign: gtk::Align::Center,
            #[name = "down_label"]
            gtk::Label {
                set_label: "↓ 0 KB/s",
                add_css_class: "net-traffic",
            },
            gtk::Separator {
                set_orientation: gtk::Orientation::Vertical,
            },
            #[name = "up_label"]
            gtk::Label {
                set_label: "↑ 0 KB/s",
                add_css_class: "net-traffic",
            },
        }
    }

    fn init(
        init: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let widgets = view_output!();
        let model = NetTraffic {
            down: String::new(),
            up: String::new(),
            down_label: widgets.down_label.clone(),
            up_label: widgets.up_label.clone(),
        };

        capsule(&root, init.grouped);

        crate::subscribe_into_msg!(
            hub::net_traffic::subscribe(),
            sender,
            NetTrafficMsg::Update
        );

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, _sender: ComponentSender<Self>) {
        match msg {
            NetTrafficMsg::Update(sample) => {
                let rx = format_rate(sample.rx_bps);
                let tx = format_rate(sample.tx_bps);
                let down = format!("↓ {rx}");
                let up = format!("↑ {tx}");

                // Coalescing: skip the GTK writes when nothing visible changed.
                if down == self.down && up == self.up {
                    return;
                }

                if down != self.down {
                    self.down = down;
                    self.down_label.set_label(&self.down);
                    set_exclusive_class(&self.down_label, rate_class(&rx), COLOR_CLASSES);
                }
                if up != self.up {
                    self.up = up;
                    self.up_label.set_label(&self.up);
                    set_exclusive_class(&self.up_label, rate_class(&tx), COLOR_CLASSES);
                }
            }
        }
    }
}

impl NamedWidget for NetTraffic {
    const NAME: &'static str = "net-traffic";
}

/// Format a bytes-per-second rate, base-1024: `KB/s` with no decimals,
/// `MB/s` / `GB/s` with one. Zero renders as `0 KB/s`.
fn format_rate(bps: f64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;

    if bps < MB {
        format!("{:.0} KB/s", bps / KB)
    } else if bps < GB {
        format!("{:.1} MB/s", bps / MB)
    } else {
        format!("{:.1} GB/s", bps / GB)
    }
}

/// `net-traffic-dim` when the formatted rate is idle (`0 KB/s`), else
/// `net-traffic-norm`.
fn rate_class(rate: &str) -> &'static str {
    if rate == "0 KB/s" {
        "net-traffic-dim"
    } else {
        "net-traffic-norm"
    }
}

#[cfg(test)]
mod tests {
    use super::format_rate;

    #[test]
    fn zero_renders_as_kb() {
        assert_eq!(format_rate(0.0), "0 KB/s");
    }

    #[test]
    fn kb_range_has_no_decimals() {
        assert_eq!(format_rate(856.0 * 1024.0), "856 KB/s");
    }

    #[test]
    fn mb_range_has_one_decimal() {
        assert_eq!(format_rate(3.4 * 1024.0 * 1024.0), "3.4 MB/s");
    }

    #[test]
    fn gb_range_has_one_decimal() {
        assert_eq!(format_rate(1.2 * 1024.0 * 1024.0 * 1024.0), "1.2 GB/s");
    }

    #[test]
    fn crosses_kb_to_mb_at_one_mb() {
        assert_eq!(format_rate(1024.0 * 1024.0 - 1.0), "1024 KB/s");
        assert_eq!(format_rate(1024.0 * 1024.0), "1.0 MB/s");
    }
}
