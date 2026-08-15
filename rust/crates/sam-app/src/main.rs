//! Steam Achievement Manager for Linux.
//!
//! One binary with two modes:
//!
//! * `sam` — the picker: browse owned apps that have achievements.
//! * `sam --app <id>` — the editor for a single app.
//!
//! # Why two modes rather than one window
//!
//! `steamclient.so` reads `SteamAppId` when it initialises. Setting it later
//! has no effect, and once a process has bound to an app it cannot rebind:
//! Steam keeps reporting the original ID. So a process can only ever act as
//! one app, and opening a game means starting a new process. The Windows
//! original solves this the same way, with a separate `SAM.Game.exe`.
//!
//! The environment variable is therefore set here, before any Steam code is
//! touched.

mod editor;
mod images;
mod library;
mod picker;

use std::process::ExitCode;

use sam_steam::{GameSchema, Steam, UserStats};

const WINDOW_TITLE: &str = "Steam Achievement Manager";

#[derive(Debug, PartialEq, Eq)]
enum Mode {
    Picker,
    Editor(u32),
}

/// Everything the command line can set.
#[derive(Debug, PartialEq, Eq)]
struct Options {
    mode: Mode,
    /// Render the window, save a PNG here, then exit. Useful for bug reports
    /// and for confirming the UI draws correctly on an unfamiliar setup.
    screenshot: Option<std::path::PathBuf>,
}

/// How long to let the UI settle before capturing, so stats have arrived from
/// Steam and the first icons have downloaded.
const SCREENSHOT_DELAY: std::time::Duration = std::time::Duration::from_secs(6);

fn main() -> ExitCode {
    let options = match parse_args(std::env::args().skip(1)) {
        Ok(Some(options)) => options,
        // --help / --version already printed.
        Ok(None) => return ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("sam: {message}");
            eprintln!("try 'sam --help'");
            return ExitCode::FAILURE;
        }
    };

    // Must happen before `Steam::load`, which dlopens the client.
    match options.mode {
        Mode::Editor(app_id) => std::env::set_var("SteamAppId", app_id.to_string()),
        Mode::Picker => std::env::remove_var("SteamAppId"),
    }

    match run(options) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("sam: {message}");
            // A desktop launcher has no terminal, so surface failures in a
            // window too. If even that fails, the stderr message stands.
            let _ = show_error_window(&message);
            ExitCode::FAILURE
        }
    }
}

fn parse_args(args: impl Iterator<Item = String>) -> Result<Option<Options>, String> {
    let mut mode = Mode::Picker;
    let mut screenshot = None;
    let mut args = args.peekable();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--screenshot" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--screenshot requires a path".to_string())?;
                screenshot = Some(std::path::PathBuf::from(value));
            }
            "-h" | "--help" => {
                print_help();
                return Ok(None);
            }
            "-V" | "--version" => {
                println!("sam {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            "--app" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--app requires an app ID".to_string())?;
                mode = Mode::Editor(parse_app_id(&value)?);
            }
            other if other.starts_with("--app=") => {
                mode = Mode::Editor(parse_app_id(&other["--app=".len()..])?);
            }
            // A bare numeric argument is treated as an app ID, matching how
            // the Windows SAM.Game.exe is invoked.
            other if other.chars().all(|c| c.is_ascii_digit()) && !other.is_empty() => {
                mode = Mode::Editor(parse_app_id(other)?);
            }
            other => return Err(format!("unrecognised argument '{other}'")),
        }
    }

    Ok(Some(Options { mode, screenshot }))
}

fn parse_app_id(value: &str) -> Result<u32, String> {
    value
        .parse::<u32>()
        .ok()
        .filter(|&id| id != 0)
        .ok_or_else(|| format!("'{value}' is not a valid app ID"))
}

fn print_help() {
    println!(
        "\
{WINDOW_TITLE} (Linux)

USAGE:
    sam                 Browse owned apps that have achievements
    sam --app <ID>      Edit achievements and stats for one app

OPTIONS:
    -h, --help          Print this help
    -V, --version       Print version
    --screenshot <PATH> Render the window, save a PNG, then exit

ENVIRONMENT:
    SAM_STEAM_ROOT      Override Steam installation discovery

Steam must be running and signed in."
    );
}

fn run(options: Options) -> Result<(), String> {
    let steam = Steam::load().map_err(|e| e.to_string())?;
    let screenshot = options.screenshot;

    match options.mode {
        Mode::Picker => {
            let session = steam.connect(None).map_err(|e| e.to_string())?;
            let app = picker::PickerApp::new(session);
            launch(
                WINDOW_TITLE.to_string(),
                [980.0, 640.0],
                Box::new(app),
                screenshot,
            )
        }
        Mode::Editor(app_id) => {
            let session = steam.connect(Some(app_id)).map_err(|e| e.to_string())?;

            let language = non_empty(session.current_game_language()).unwrap_or("english".into());
            let schema_path = session.paths().schema_path(app_id);
            // A missing schema is not fatal: Steam writes it on first launch,
            // so a never-played game simply has no metadata to show yet.
            let schema = GameSchema::load(&schema_path, app_id, &language).unwrap_or_default();

            // Verify the vtable once, here, so a layout problem surfaces
            // before any window opens rather than on the first click.
            let stats = UserStats::new(&session).map_err(|e| e.to_string())?;
            let order = stats.order();
            let confidence = stats.confidence().clone();

            let title = session
                .app_name(app_id)
                .map(|name| format!("{WINDOW_TITLE} | {name}"))
                .unwrap_or_else(|| format!("{WINDOW_TITLE} | {app_id}"));

            let app = editor::EditorApp::new(session, app_id, schema, order, confidence);
            launch(title, [900.0, 680.0], Box::new(app), screenshot)
        }
    }
}

fn launch(
    title: String,
    size: [f32; 2],
    app: Box<dyn eframe::App>,
    screenshot: Option<std::path::PathBuf>,
) -> Result<(), String> {
    let app: Box<dyn eframe::App> = match screenshot {
        Some(path) => Box::new(ScreenshotHarness::new(app, path)),
        None => app,
    };

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(size)
            .with_min_inner_size([560.0, 400.0])
            .with_app_id("steam-achievement-manager"),
        ..Default::default()
    };

    eframe::run_native(
        &title,
        options,
        Box::new(move |cc| {
            theme(&cc.egui_ctx);
            Ok(app)
        }),
    )
    .map_err(|e| format!("could not open a window: {e}"))
}

/// A dark palette in the spirit of the original, which drew its achievement
/// list on a near-black background.
fn theme(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = egui::Color32::from_rgb(24, 26, 30);
    visuals.window_fill = egui::Color32::from_rgb(28, 30, 35);
    visuals.extreme_bg_color = egui::Color32::from_rgb(18, 19, 23);
    visuals.widgets.noninteractive.bg_fill = egui::Color32::from_rgb(34, 37, 43);
    visuals.selection.bg_fill = egui::Color32::from_rgb(52, 101, 164);
    ctx.set_visuals(visuals);

    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(10.0, 5.0);
    ctx.set_style(style);
}

/// Wraps a real app, captures the window once it has settled, then quits.
///
/// Uses egui's own capture rather than an external screenshot tool, so it
/// works the same under X11, Wayland and a headless compositor.
struct ScreenshotHarness {
    inner: Box<dyn eframe::App>,
    path: std::path::PathBuf,
    start: std::time::Instant,
    requested: bool,
}

impl ScreenshotHarness {
    fn new(inner: Box<dyn eframe::App>, path: std::path::PathBuf) -> Self {
        Self {
            inner,
            path,
            start: std::time::Instant::now(),
            requested: false,
        }
    }
}

impl eframe::App for ScreenshotHarness {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.inner.update(ctx, frame);

        if !self.requested && self.start.elapsed() >= SCREENSHOT_DELAY {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
            self.requested = true;
        }

        let captured = ctx.input(|i| {
            i.events.iter().find_map(|event| match event {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });

        if let Some(image) = captured {
            match save_screenshot(&image, &self.path) {
                Ok(()) => eprintln!("sam: wrote {}", self.path.display()),
                Err(e) => eprintln!("sam: could not write screenshot: {e}"),
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        // The delay is wall-clock, so keep frames coming even when idle.
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
}

fn save_screenshot(image: &egui::ColorImage, path: &std::path::Path) -> Result<(), String> {
    let [width, height] = image.size;
    let mut buffer = image::RgbaImage::new(width as u32, height as u32);
    for (pixel, source) in buffer.pixels_mut().zip(image.pixels.iter()) {
        *pixel = image::Rgba(source.to_array());
    }
    buffer.save(path).map_err(|e| e.to_string())
}

/// Report a startup failure in a window, for launcher and desktop-file users.
fn show_error_window(message: &str) -> Result<(), eframe::Error> {
    let message = message.to_string();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([560.0, 260.0]),
        ..Default::default()
    };

    eframe::run_simple_native(WINDOW_TITLE, options, move |ctx, _frame| {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(12.0);
            ui.heading("Could not start");
            ui.add_space(8.0);
            ui.label(&message);
            ui.add_space(12.0);
            ui.label(
                egui::RichText::new(
                    "Steam must be running and signed in before starting this tool.",
                )
                .weak(),
            );
            ui.add_space(12.0);
            if ui.button("Close").clicked() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        });
    })
}

fn non_empty(value: String) -> Option<String> {
    (!value.trim().is_empty()).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Option<Options>, String> {
        parse_args(args.iter().map(|s| s.to_string()))
    }

    fn mode_of(args: &[&str]) -> Mode {
        parse(args).expect("should parse").expect("should run").mode
    }

    #[test]
    fn no_arguments_selects_the_picker() {
        assert_eq!(mode_of(&[]), Mode::Picker);
    }

    #[test]
    fn app_flag_selects_the_editor() {
        assert_eq!(mode_of(&["--app", "480"]), Mode::Editor(480));
        assert_eq!(mode_of(&["--app=480"]), Mode::Editor(480));
    }

    #[test]
    fn bare_number_is_an_app_id_like_the_windows_version() {
        assert_eq!(mode_of(&["620"]), Mode::Editor(620));
    }

    #[test]
    fn screenshot_flag_is_independent_of_mode() {
        let options = parse(&["--app", "620", "--screenshot", "/tmp/x.png"])
            .unwrap()
            .unwrap();
        assert_eq!(options.mode, Mode::Editor(620));
        assert_eq!(options.screenshot.unwrap().to_str(), Some("/tmp/x.png"));
        // And it works without an app, for the picker.
        let options = parse(&["--screenshot", "/tmp/x.png"]).unwrap().unwrap();
        assert_eq!(options.mode, Mode::Picker);
        assert!(options.screenshot.is_some());
    }

    #[test]
    fn screenshot_requires_a_path() {
        assert!(parse(&["--screenshot"]).is_err());
    }

    #[test]
    fn rejects_bad_app_ids() {
        assert!(parse(&["--app", "nope"]).is_err());
        assert!(parse(&["--app", "0"]).is_err());
        assert!(parse(&["--app"]).is_err());
        assert!(parse(&["--nonsense"]).is_err());
    }
}
