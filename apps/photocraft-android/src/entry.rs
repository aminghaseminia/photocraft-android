#![allow(unsafe_code)]
//! Android's entry point: the one place this shell needs `unsafe` (the `#[unsafe(no_mangle)]`
//! export that `android-activity`'s GameActivity glue looks up).

use eframe::egui;
use photocraft_engine::Session;
use photocraft_ui_egui::PhotocraftApp;
use photocraft_ui_egui::theme::ThemeKind;
use winit::platform::android::activity::AndroidApp;

/// Called by `android-activity` on its own thread.
///
/// It can be called again if the activity is destroyed and recreated, so anything global is
/// initialized once and any state that must survive belongs in the app's own persistence.
#[unsafe(no_mangle)]
fn android_main(app: AndroidApp) {
    init_logging();
    if let Err(error) = photocraft_android_jni::init() {
        log::error!("JNI: {error}");
    }

    let options = eframe::NativeOptions {
        android_app: Some(app),
        viewport: egui::ViewportBuilder::default().with_title("PhotoCraft").with_inner_size([1280.0, 800.0]),
        renderer: eframe::Renderer::Wgpu,
        // PhotoCraft owns its fallback fonts (`cjk_fonts`, craft-fonts), as on every platform.
        system_font_fallback: false,
        // eframe's own window/panel state. The app's preferences go through `Services`.
        persistence_path: crate::services::config_dir().map(|dir| dir.join("ui.ron")),
        ..Default::default()
    };

    let result = eframe::run_native(
        "PhotoCraft",
        options,
        Box::new(|cc| {
            PhotocraftApp::setup_context(&cc.egui_ctx, ThemeKind::Pro);
            let mut services = crate::services::services();
            // Brush presets load on a worker thread; the app attaches them when they arrive.
            services.preset_store = crate::services::presets();
            let mut app = PhotocraftApp::new(Session::new(), services);
            // Android has no OS title bar, no window menu and no printing spooler.
            app.custom_titlebar = false;
            app.integrated_titlebar = false;
            // Long commands and file opens run as background jobs with progress and Cancel.
            app.background_jobs = true;
            // The GPU canvas shares the window's wgpu device; without one the CPU compositor runs.
            if let Some(render_state) = cc.wgpu_render_state.clone() {
                app.set_wgpu(render_state);
            }
            app.set_theme(&cc.egui_ctx, ThemeKind::Pro);
            Ok(Box::new(app))
        }),
    );

    if let Err(error) = result {
        log::error!("PhotoCraft failed to start: {error}");
    }
}

fn init_logging() {
    use std::sync::Once;
    static LOGGING: Once = Once::new();
    LOGGING.call_once(|| {
        android_logger::init_once(android_logger::Config::default().with_max_level(log::LevelFilter::Info));
        log::info!("PhotoCraft {} starting", env!("CARGO_PKG_VERSION"));
    });
}
