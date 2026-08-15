//! The game picker.

use std::collections::VecDeque;
use std::process::Command;
use std::sync::mpsc::Receiver;

use sam_steam::{CallbackEvent, CallbackPump, Session};

use crate::images::ImageStore;
use crate::library::{self, GameEntry, RemoteApp, Source};

/// Remote candidates to test for ownership per frame.
///
/// Each check is an IPC round trip to Steam, and the master list has tens of
/// thousands of entries, so this is spread across frames to keep the window
/// responsive instead of freezing for several seconds.
const OWNERSHIP_CHECKS_PER_FRAME: usize = 200;

const CELL_WIDTH: f32 = 208.0;
const CAPSULE_HEIGHT: f32 = 78.0;

pub struct PickerApp {
    session: Session,
    games: Vec<GameEntry>,
    images: ImageStore,

    search: String,
    hide_without_schema: bool,
    add_id: String,

    status: String,
    error: Option<String>,

    remote: Option<Receiver<Result<Vec<RemoteApp>, String>>>,
    pending_remote: VecDeque<RemoteApp>,
    remote_seen: usize,
    remote_total: usize,
}

impl PickerApp {
    pub fn new(session: Session) -> Self {
        let mut app = Self {
            session,
            games: Vec::new(),
            images: ImageStore::new(),
            search: String::new(),
            hide_without_schema: true,
            add_id: String::new(),
            status: String::new(),
            error: None,
            remote: None,
            pending_remote: VecDeque::new(),
            remote_seen: 0,
            remote_total: 0,
        };
        app.rescan_local();
        app
    }

    /// Rebuild the list from local sources only. Fast and offline.
    fn rescan_local(&mut self) {
        self.games.clear();
        let candidates = library::local_candidates(self.session.paths());

        for app_id in candidates {
            if !self.session.owns_app(app_id) {
                continue;
            }
            let has_schema = self.session.paths().schema_path(app_id).is_file();
            let source = if has_schema {
                Source::CachedSchema
            } else {
                Source::Installed
            };
            self.games
                .push(library::describe(&self.session, app_id, "normal", source));
        }

        self.sort_games();
        self.status = format!("{} games found locally.", self.games.len());
    }

    fn sort_games(&mut self) {
        self.games.sort_by_key(|g| g.name.to_lowercase());
        self.games.dedup_by_key(|g| g.app_id);
    }

    fn start_remote_scan(&mut self) {
        if self.remote.is_some() || !self.pending_remote.is_empty() {
            return;
        }
        self.error = None;
        self.status = "Downloading the full game list...".to_string();
        self.remote = Some(library::spawn_remote_fetch());
    }

    /// Collect the remote list once it arrives, then work through it a slice
    /// at a time so the UI keeps painting.
    fn drive_remote_scan(&mut self) {
        if let Some(rx) = &self.remote {
            match rx.try_recv() {
                Ok(Ok(apps)) => {
                    self.remote_total = apps.len();
                    self.remote_seen = 0;
                    self.pending_remote = apps.into();
                    self.remote = None;
                }
                Ok(Err(message)) => {
                    self.error = Some(message);
                    self.status = "Showing locally discovered games.".to_string();
                    self.remote = None;
                }
                // Still downloading.
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => self.remote = None,
            }
        }

        if self.pending_remote.is_empty() {
            return;
        }

        let mut added = false;
        for _ in 0..OWNERSHIP_CHECKS_PER_FRAME {
            let Some(candidate) = self.pending_remote.pop_front() else {
                break;
            };
            self.remote_seen += 1;

            if self.games.iter().any(|g| g.app_id == candidate.app_id) {
                continue;
            }
            if !self.session.owns_app(candidate.app_id) {
                continue;
            }
            self.games.push(library::describe(
                &self.session,
                candidate.app_id,
                &candidate.kind,
                Source::Remote,
            ));
            added = true;
        }

        if added {
            self.sort_games();
        }

        self.status = if self.pending_remote.is_empty() {
            format!("{} games.", self.games.len())
        } else {
            format!(
                "Checking ownership... {}/{} ({} games so far)",
                self.remote_seen,
                self.remote_total,
                self.games.len()
            )
        };
    }

    /// Names and capsules arrive asynchronously; refresh entries as Steam
    /// reports metadata for them.
    fn pump_callbacks(&mut self) {
        let events = CallbackPump::new(&self.session).poll();
        for event in events {
            if let CallbackEvent::AppDataChanged { app_id, result } = event {
                if !result {
                    continue;
                }
                if let Some(index) = self.games.iter().position(|g| g.app_id == app_id) {
                    let kind = self.games[index].kind.clone();
                    let source = self.games[index].source;
                    self.games[index] = library::describe(&self.session, app_id, &kind, source);
                }
            }
        }
    }

    fn add_by_id(&mut self) {
        let text = self.add_id.trim().to_string();
        let Ok(app_id) = text.parse::<u32>() else {
            self.error = Some(format!("'{text}' is not a valid app ID."));
            return;
        };
        if !self.session.owns_app(app_id) {
            self.error = Some(format!("You do not own app {app_id}."));
            return;
        }
        if self.games.iter().any(|g| g.app_id == app_id) {
            self.error = None;
            self.search = app_id.to_string();
            self.add_id.clear();
            return;
        }

        self.games.push(library::describe(
            &self.session,
            app_id,
            "normal",
            Source::Remote,
        ));
        self.sort_games();
        self.error = None;
        self.add_id.clear();
    }

    fn matches(&self, game: &GameEntry) -> bool {
        if self.hide_without_schema && !game.has_schema {
            return false;
        }
        let needle = self.search.trim();
        if needle.is_empty() {
            return true;
        }
        game.name.to_lowercase().contains(&needle.to_lowercase())
            || game.app_id.to_string().contains(needle)
    }

    /// Open the editor for `app_id` in a new process.
    ///
    /// `SteamAppId` is set on the child explicitly rather than left to the
    /// child's own startup, so it is present from the moment it execs and
    /// cannot be missed.
    fn open_editor(&mut self, app_id: u32) {
        let exe = match std::env::current_exe() {
            Ok(exe) => exe,
            Err(e) => {
                self.error = Some(format!("Could not locate this executable: {e}"));
                return;
            }
        };

        match Command::new(exe)
            .arg("--app")
            .arg(app_id.to_string())
            .env("SteamAppId", app_id.to_string())
            .spawn()
        {
            Ok(_) => self.error = None,
            Err(e) => self.error = Some(format!("Could not open the editor: {e}")),
        }
    }
}

impl eframe::App for PickerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.pump_callbacks();
        self.drive_remote_scan();
        self.images.pump(ctx);

        // Steam callbacks and downloads arrive off-frame, so keep animating
        // while anything is outstanding.
        if !self.pending_remote.is_empty() || self.remote.is_some() || self.images.outstanding() > 0
        {
            ctx.request_repaint();
        }

        let mut to_open: Option<u32> = None;

        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label("Search");
                ui.add(
                    egui::TextEdit::singleline(&mut self.search)
                        .desired_width(220.0)
                        .hint_text("name or app ID"),
                );
                if ui.button("Clear").clicked() {
                    self.search.clear();
                }

                ui.separator();
                ui.checkbox(
                    &mut self.hide_without_schema,
                    "Only games with achievements",
                )
                .on_hover_text(
                    "Steam caches an achievement schema the first time a game runs. \
                         Games without one have nothing to edit yet.",
                );

                ui.separator();
                if ui.button("Rescan").clicked() {
                    self.rescan_local();
                }
                let scanning = self.remote.is_some() || !self.pending_remote.is_empty();
                if ui
                    .add_enabled(!scanning, egui::Button::new("Find more…"))
                    .on_hover_text(
                        "Download the full app list and check every entry against your \
                         library. Finds owned games you have never launched.",
                    )
                    .clicked()
                {
                    self.start_remote_scan();
                }
            });

            ui.horizontal(|ui| {
                ui.label("Add by app ID");
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.add_id)
                        .desired_width(120.0)
                        .hint_text("e.g. 620"),
                );
                let submitted =
                    response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if ui.button("Add").clicked() || submitted {
                    self.add_by_id();
                }
            });
            ui.add_space(6.0);
        });

        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(&self.status);
                let outstanding = self.images.outstanding();
                if outstanding > 0 {
                    ui.separator();
                    ui.label(format!("Downloading {outstanding} images..."));
                }
            });
            if let Some(error) = &self.error {
                ui.colored_label(egui::Color32::from_rgb(232, 120, 120), error);
            }
            ui.add_space(4.0);
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            let visible: Vec<usize> = (0..self.games.len())
                .filter(|&i| self.matches(&self.games[i]))
                .collect();

            if visible.is_empty() {
                ui.centered_and_justified(|ui| {
                    ui.label(if self.games.is_empty() {
                        "No games found. Try \"Find more…\" to search your whole library."
                    } else {
                        "No games match the current filter."
                    });
                });
                return;
            }

            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for index in visible {
                        // Cloned so the immutable borrow of `self.games` ends
                        // before `self.images` is borrowed mutably below.
                        let game = self.games[index].clone();
                        if self.game_card(ui, &game) {
                            to_open = Some(game.app_id);
                        }
                    }
                });
            });
        });

        if let Some(app_id) = to_open {
            self.open_editor(app_id);
        }
    }
}

impl PickerApp {
    /// Draw one game. Returns true when it was activated.
    fn game_card(&mut self, ui: &mut egui::Ui, game: &GameEntry) -> bool {
        let mut clicked = false;

        ui.allocate_ui(egui::vec2(CELL_WIDTH, CAPSULE_HEIGHT + 46.0), |ui| {
            let response = egui::Frame::group(ui.style())
                .inner_margin(6.0)
                .show(ui, |ui| {
                    ui.vertical(|ui| {
                        ui.set_width(CELL_WIDTH - 24.0);

                        let texture = game
                            .capsule_url
                            .as_ref()
                            .and_then(|url| self.images.texture(url))
                            .cloned();

                        match texture {
                            Some(handle) => {
                                ui.add(
                                    egui::Image::new(&handle)
                                        .fit_to_exact_size(egui::vec2(
                                            CELL_WIDTH - 24.0,
                                            CAPSULE_HEIGHT,
                                        ))
                                        .corner_radius(3.0),
                                );
                            }
                            None => {
                                let (rect, _) = ui.allocate_exact_size(
                                    egui::vec2(CELL_WIDTH - 24.0, CAPSULE_HEIGHT),
                                    egui::Sense::hover(),
                                );
                                ui.painter().rect_filled(
                                    rect,
                                    3.0,
                                    egui::Color32::from_rgb(44, 47, 54),
                                );
                            }
                        }

                        ui.add_space(4.0);
                        ui.label(egui::RichText::new(&game.name).strong().size(12.5));

                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(game.app_id.to_string())
                                    .weak()
                                    .size(11.0),
                            );
                            if !game.has_schema {
                                ui.label(
                                    egui::RichText::new("no schema")
                                        .size(11.0)
                                        .color(egui::Color32::from_rgb(200, 160, 90)),
                                )
                                .on_hover_text(
                                    "Steam has not cached achievements for this game. \
                                     Run it once, then rescan.",
                                );
                            }
                        });
                    });
                })
                .response;

            let response = response.interact(egui::Sense::click());
            if response.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if response.clicked() || response.double_clicked() {
                clicked = true;
            }
        });

        clicked
    }
}
