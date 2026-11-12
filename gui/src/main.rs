//! docver — local version control for Office documents, with a redline view.
//!
//! Native, no runtime, no browser: the whole app is one binary and one git
//! repository. Nothing here talks to a network.

mod app;
mod engine;
mod theme;
mod view;

use eframe::egui;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("docver")
            .with_app_id("docver")
            .with_inner_size([1280.0, 860.0])
            .with_min_inner_size([860.0, 560.0])
            .with_drag_and_drop(true),
        // Multisampling is off: this is a 2D text app, and MSAA on a large
        // scrolled galley costs fill rate for edges that are already crisp.
        multisampling: 0,
        ..Default::default()
    };

    eframe::run_native(
        "docver",
        options,
        Box::new(|cc| Ok(Box::new(app::App::new(cc)))),
    )
}
