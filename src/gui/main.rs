//! Native desktop PDF viewer entry point (`cargo run --bin rust-pdf-gui --features native-gui`).

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_title("rust-pdf"),
        ..Default::default()
    };

    eframe::run_native(
        "rust-pdf",
        options,
        Box::new(|cc| Ok(Box::new(rust_pdf::gui::PdfViewerApp::new(cc)))),
    )
}
