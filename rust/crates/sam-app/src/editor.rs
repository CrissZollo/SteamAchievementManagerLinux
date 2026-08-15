//! The per-app achievement and statistic editor.

use sam_steam::{
    schema::is_protected, CallbackEvent, CallbackPump, GameSchema, OrderConfidence, OverloadOrder,
    Session, StatBounds, UserStats,
};

use crate::images::ImageStore;
use crate::library;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Achievements,
    Statistics,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Filter {
    All,
    Locked,
    Unlocked,
}

/// Where the destructive "reset everything" flow has got to.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ResetStage {
    Idle,
    /// Asking whether to proceed, and whether achievements go too.
    Confirming {
        achievements_too: bool,
    },
}

struct AchievementRow {
    id: String,
    name: String,
    description: String,
    icon_unlocked: String,
    icon_locked: String,
    permission: i32,
    hidden: bool,
    /// State as Steam reports it.
    original: bool,
    /// State as edited in the UI.
    current: bool,
    unlock_time: u32,
}

impl AchievementRow {
    fn is_modified(&self) -> bool {
        self.current != self.original
    }

    fn is_protected(&self) -> bool {
        is_protected(self.permission)
    }
}

enum StatValue {
    Integer { original: i32 },
    Float { original: f32 },
}

struct StatRow {
    id: String,
    display_name: String,
    value: StatValue,
    /// Free text, so a half-typed number does not clobber the stat.
    text: String,
    permission: i32,
    increment_only: bool,
    /// Set when `text` cannot be parsed or violates the schema bounds.
    problem: Option<String>,
}

impl StatRow {
    fn is_protected(&self) -> bool {
        is_protected(self.permission)
    }

    fn original_text(&self) -> String {
        match self.value {
            StatValue::Integer { original } => original.to_string(),
            StatValue::Float { original } => original.to_string(),
        }
    }

    fn is_modified(&self) -> bool {
        self.text.trim() != self.original_text()
    }
}

pub struct EditorApp {
    session: Session,
    app_id: u32,
    schema: GameSchema,
    order: OverloadOrder,
    confidence: OrderConfidence,

    achievements: Vec<AchievementRow>,
    stats: Vec<StatRow>,
    images: ImageStore,

    tab: Tab,
    filter: Filter,
    search: String,
    allow_stat_edits: bool,
    show_icons: bool,

    loading: bool,
    loaded_once: bool,
    status: String,
    error: Option<String>,
    notice: Option<String>,
    reset: ResetStage,
}

impl EditorApp {
    pub fn new(
        session: Session,
        app_id: u32,
        schema: GameSchema,
        order: OverloadOrder,
        confidence: OrderConfidence,
    ) -> Self {
        let mut app = Self {
            session,
            app_id,
            schema,
            order,
            confidence,
            achievements: Vec::new(),
            stats: Vec::new(),
            images: ImageStore::new(),
            tab: Tab::Achievements,
            filter: Filter::All,
            search: String::new(),
            allow_stat_edits: false,
            show_icons: true,
            loading: false,
            loaded_once: false,
            status: String::new(),
            error: None,
            notice: None,
            reset: ResetStage::Idle,
        };
        app.request_reload();
        app
    }

    fn stats_api(&self) -> UserStats<'_> {
        UserStats::reuse(&self.session, self.order, self.confidence.clone())
    }

    fn request_reload(&mut self) {
        self.loading = true;
        self.error = None;
        self.notice = None;
        self.status = "Retrieving stat information...".to_string();

        let steam_id = self.session.steam_id();
        let handle = self.stats_api().request_user_stats(steam_id);
        if handle == 0 {
            self.loading = false;
            self.error = Some(
                "Steam refused the stats request. Make sure the client is signed in.".to_string(),
            );
        }
    }

    /// Populate rows once Steam reports the data has arrived.
    fn load_rows(&mut self) {
        // The overload order can only be proved once real stat values exist,
        // so re-resolve on every load rather than trusting the startup guess.
        let resolution = {
            let mut api = self.stats_api();
            let resolution = api.resolve_overload_order(
                &self.schema.integer_stat_names(),
                &self.schema.float_stat_names(),
            );
            self.order = resolution.order;
            self.confidence = resolution.confidence.clone();
            resolution
        };

        // `api` borrows `self`, so the rows are built inside a scope and only
        // moved into place once that borrow has ended.
        let (achievements, stats) = {
            let api = self.stats_api();

            let achievements: Vec<AchievementRow> = self
                .schema
                .achievements
                .iter()
                .filter_map(|def| {
                    let (unlocked, unlock_time) = api.achievement_and_unlock_time(&def.id)?;
                    Some(AchievementRow {
                        id: def.id.clone(),
                        name: if def.name.starts_with('#') || def.name.is_empty() {
                            // Unresolved localisation token; the ID is more useful.
                            def.id.clone()
                        } else {
                            def.name.clone()
                        },
                        description: def.description.clone(),
                        icon_unlocked: def.icon_normal.clone(),
                        icon_locked: if def.icon_locked.is_empty() {
                            def.icon_normal.clone()
                        } else {
                            def.icon_locked.clone()
                        },
                        permission: def.permission,
                        hidden: def.hidden,
                        original: unlocked,
                        current: unlocked,
                        unlock_time,
                    })
                })
                .collect();

            let stats: Vec<StatRow> = self
                .schema
                .stats
                .iter()
                .filter_map(|def| {
                    let (value, text) = match def.bounds {
                        StatBounds::Integer { .. } => {
                            let current = api.stat_i32(&def.id)?;
                            (
                                StatValue::Integer { original: current },
                                current.to_string(),
                            )
                        }
                        StatBounds::Float { .. } => {
                            let current = api.stat_f32(&def.id)?;
                            (StatValue::Float { original: current }, current.to_string())
                        }
                    };
                    Some(StatRow {
                        id: def.id.clone(),
                        display_name: def.display_name.clone(),
                        value,
                        text,
                        permission: def.permission,
                        increment_only: def.increment_only,
                        problem: None,
                    })
                })
                .collect();

            (achievements, stats)
        };

        self.achievements = achievements;
        self.stats = stats;

        self.loading = false;
        self.loaded_once = true;
        self.status = format!(
            "Retrieved {} achievements and {} statistics.",
            self.achievements.len(),
            self.stats.len()
        );

        // Surface an unproven overload order: reads are fine, writes less so.
        if let OrderConfidence::Assumed(reason) = &resolution.confidence {
            if !self.stats.is_empty() {
                self.notice = Some(format!(
                    "Stat type ordering could not be verified ({reason}). \
                     Achievements are unaffected; take care when editing statistics."
                ));
            }
        }
    }

    fn pump(&mut self, ctx: &egui::Context) {
        let events = CallbackPump::new(&self.session).poll();
        for event in events {
            match event {
                CallbackEvent::UserStatsReceived {
                    result, game_id, ..
                } => {
                    if game_id != 0 && game_id as u32 != self.app_id {
                        continue;
                    }
                    if result == 1 {
                        self.load_rows();
                    } else {
                        self.loading = false;
                        self.error = Some(translate_result(result));
                    }
                }
                CallbackEvent::UserStatsStored { result, .. } if result != 1 => {
                    self.error = Some(format!(
                        "Steam rejected the store ({}).",
                        translate_result(result)
                    ));
                }
                _ => {}
            }
        }

        if self.images.pump(ctx) || self.loading || self.images.outstanding() > 0 {
            ctx.request_repaint();
        }
    }

    fn commit(&mut self) {
        self.error = None;
        self.notice = None;

        // Validate every edited stat before writing anything, so a typo in one
        // field cannot leave the rest half-applied.
        let mut parsed: Vec<(usize, StatWrite)> = Vec::new();
        for (index, row) in self.stats.iter_mut().enumerate() {
            row.problem = None;
            if !row.is_modified() {
                continue;
            }
            if row.is_protected() {
                row.problem = Some("protected".into());
                continue;
            }
            let text = row.text.trim();
            match row.value {
                StatValue::Integer { original } => match text.parse::<i32>() {
                    Ok(v) => {
                        if row.increment_only && v < original {
                            row.problem = Some("increment only".into());
                        } else {
                            parsed.push((index, StatWrite::Integer(v)));
                        }
                    }
                    Err(_) => row.problem = Some("not a whole number".into()),
                },
                StatValue::Float { original } => match text.parse::<f32>() {
                    Ok(v) => {
                        if row.increment_only && v < original {
                            row.problem = Some("increment only".into());
                        } else {
                            parsed.push((index, StatWrite::Float(v)));
                        }
                    }
                    Err(_) => row.problem = Some("not a number".into()),
                },
            }
        }

        if self.stats.iter().any(|s| s.problem.is_some()) {
            self.error =
                Some("Some statistics could not be applied; see the highlighted rows.".into());
            return;
        }

        let api = self.stats_api();
        let mut achievement_writes = 0usize;

        for row in &self.achievements {
            if !row.is_modified() || row.is_protected() {
                continue;
            }
            if !api.set_achievement(&row.id, row.current) {
                self.error = Some(format!("Failed to set '{}'. Nothing was stored.", row.name));
                return;
            }
            achievement_writes += 1;
        }

        let mut stat_writes = 0usize;
        for (index, write) in &parsed {
            let row = &self.stats[*index];
            let ok = match write {
                StatWrite::Integer(v) => api.set_stat_i32(&row.id, *v),
                StatWrite::Float(v) => api.set_stat_f32(&row.id, *v),
            };
            if !ok {
                self.error = Some(format!(
                    "Failed to set '{}'. Nothing was stored.",
                    row.display_name
                ));
                return;
            }
            stat_writes += 1;
        }

        if achievement_writes == 0 && stat_writes == 0 {
            self.notice = Some("Nothing to store.".into());
            return;
        }

        if !api.store_stats() {
            self.error = Some("Steam rejected the store. Your changes were not saved.".into());
            return;
        }

        self.notice = Some(format!(
            "Stored {achievement_writes} achievements and {stat_writes} statistics."
        ));
        self.request_reload();
    }

    fn perform_reset(&mut self, achievements_too: bool) {
        let ok = self.stats_api().reset_all_stats(achievements_too);
        self.reset = ResetStage::Idle;
        if ok {
            self.notice = Some(if achievements_too {
                "Reset all statistics and achievements.".into()
            } else {
                "Reset all statistics.".into()
            });
            self.request_reload();
        } else {
            self.error = Some("Steam refused the reset.".into());
        }
    }

    fn visible_achievements(&self) -> Vec<usize> {
        let needle = self.search.trim().to_lowercase();
        (0..self.achievements.len())
            .filter(|&i| {
                let row = &self.achievements[i];
                let by_filter = match self.filter {
                    Filter::All => true,
                    Filter::Locked => !row.current,
                    Filter::Unlocked => row.current,
                };
                if !by_filter {
                    return false;
                }
                if needle.is_empty() {
                    return true;
                }
                row.name.to_lowercase().contains(&needle)
                    || row.description.to_lowercase().contains(&needle)
                    || row.id.to_lowercase().contains(&needle)
            })
            .collect()
    }

    fn pending_changes(&self) -> usize {
        self.achievements.iter().filter(|a| a.is_modified()).count()
            + self.stats.iter().filter(|s| s.is_modified()).count()
    }
}

enum StatWrite {
    Integer(i32),
    Float(f32),
}

impl eframe::App for EditorApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.pump(ctx);

        self.toolbar(ctx);
        self.status_bar(ctx);
        self.reset_dialog(ctx);

        egui::CentralPanel::default().show(ctx, |ui| {
            if self.loading && !self.loaded_once {
                ui.centered_and_justified(|ui| {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Waiting for Steam to send this game's stats...");
                    });
                });
                return;
            }

            match self.tab {
                Tab::Achievements => self.achievements_tab(ui),
                Tab::Statistics => self.statistics_tab(ui),
            }
        });
    }
}

impl EditorApp {
    fn toolbar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.tab, Tab::Achievements, "Achievements");
                ui.selectable_value(&mut self.tab, Tab::Statistics, "Statistics");

                ui.separator();

                let busy = self.loading;
                if ui.add_enabled(!busy, egui::Button::new("Reload")).clicked() {
                    self.request_reload();
                }

                let pending = self.pending_changes();
                let store = egui::Button::new(if pending > 0 {
                    format!("Store ({pending})")
                } else {
                    "Store".to_string()
                });
                if ui.add_enabled(!busy && pending > 0, store).clicked() {
                    self.commit();
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .button("Reset all stats…")
                        .on_hover_text("Wipes every statistic for this game.")
                        .clicked()
                    {
                        self.reset = ResetStage::Confirming {
                            achievements_too: false,
                        };
                    }
                });
            });

            if self.tab == Tab::Achievements {
                ui.horizontal(|ui| {
                    ui.label("Filter");
                    ui.selectable_value(&mut self.filter, Filter::All, "All");
                    ui.selectable_value(&mut self.filter, Filter::Locked, "Locked");
                    ui.selectable_value(&mut self.filter, Filter::Unlocked, "Unlocked");

                    ui.separator();
                    ui.add(
                        egui::TextEdit::singleline(&mut self.search)
                            .desired_width(200.0)
                            .hint_text("search"),
                    );
                    ui.checkbox(&mut self.show_icons, "Icons");

                    ui.separator();
                    if ui.button("Unlock all").clicked() {
                        for row in self.achievements.iter_mut().filter(|r| !r.is_protected()) {
                            row.current = true;
                        }
                    }
                    if ui.button("Lock all").clicked() {
                        for row in self.achievements.iter_mut().filter(|r| !r.is_protected()) {
                            row.current = false;
                        }
                    }
                    if ui.button("Invert").clicked() {
                        for row in self.achievements.iter_mut().filter(|r| !r.is_protected()) {
                            row.current = !row.current;
                        }
                    }
                    if ui.button("Revert").clicked() {
                        for row in self.achievements.iter_mut() {
                            row.current = row.original;
                        }
                        for row in self.stats.iter_mut() {
                            row.text = row.original_text();
                            row.problem = None;
                        }
                    }
                });
            }
            ui.add_space(6.0);
        });
    }

    fn status_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if self.loading {
                    ui.spinner();
                }
                ui.label(&self.status);
                let outstanding = self.images.outstanding();
                if outstanding > 0 {
                    ui.separator();
                    ui.label(format!("Downloading {outstanding} icons..."));
                }
            });
            if let Some(notice) = self.notice.clone() {
                ui.colored_label(egui::Color32::from_rgb(140, 200, 140), notice);
            }
            if let Some(error) = self.error.clone() {
                ui.colored_label(egui::Color32::from_rgb(232, 120, 120), error);
            }
            ui.add_space(4.0);
        });
    }

    fn reset_dialog(&mut self, ctx: &egui::Context) {
        let ResetStage::Confirming { achievements_too } = self.reset else {
            return;
        };
        let mut achievements_too = achievements_too;
        let mut action: Option<bool> = None;
        let mut cancel = false;

        egui::Window::new("Reset all statistics")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.label("This wipes every statistic for this game on Steam's servers.");
                ui.label(
                    egui::RichText::new("It cannot be undone.")
                        .color(egui::Color32::from_rgb(232, 160, 120)),
                );
                ui.add_space(8.0);
                ui.checkbox(&mut achievements_too, "Also reset achievements");
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                    let label = if achievements_too {
                        "Reset stats and achievements"
                    } else {
                        "Reset stats"
                    };
                    if ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new(label).color(egui::Color32::WHITE),
                            )
                            .fill(egui::Color32::from_rgb(150, 60, 60)),
                        )
                        .clicked()
                    {
                        action = Some(achievements_too);
                    }
                });
            });

        if cancel {
            self.reset = ResetStage::Idle;
        } else if let Some(too) = action {
            self.perform_reset(too);
        } else {
            self.reset = ResetStage::Confirming { achievements_too };
        }
    }

    fn achievements_tab(&mut self, ui: &mut egui::Ui) {
        let visible = self.visible_achievements();

        if visible.is_empty() {
            ui.centered_and_justified(|ui| {
                ui.label(if self.achievements.is_empty() {
                    "This game reports no achievements."
                } else {
                    "No achievements match the current filter."
                });
            });
            return;
        }

        let app_id = self.app_id;
        let show_icons = self.show_icons;
        let row_height = if show_icons { 56.0 } else { 34.0 };

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show_rows(ui, row_height, visible.len(), |ui, range| {
                for position in range {
                    let index = visible[position];

                    // Icon URLs are read before the mutable borrow below.
                    let (icon_file, protected, modified) = {
                        let row = &self.achievements[index];
                        (
                            if row.current {
                                row.icon_unlocked.clone()
                            } else {
                                row.icon_locked.clone()
                            },
                            row.is_protected(),
                            row.is_modified(),
                        )
                    };

                    let texture = if show_icons && !icon_file.is_empty() {
                        let url = library::community_asset_url(app_id, &icon_file);
                        self.images.texture(&url).cloned()
                    } else {
                        None
                    };

                    let row = &mut self.achievements[index];

                    ui.horizontal(|ui| {
                        if show_icons {
                            match &texture {
                                Some(handle) => {
                                    ui.add(
                                        egui::Image::new(handle)
                                            .fit_to_exact_size(egui::vec2(48.0, 48.0))
                                            .corner_radius(3.0),
                                    );
                                }
                                None => {
                                    let (rect, _) = ui.allocate_exact_size(
                                        egui::vec2(48.0, 48.0),
                                        egui::Sense::hover(),
                                    );
                                    ui.painter().rect_filled(
                                        rect,
                                        3.0,
                                        egui::Color32::from_rgb(44, 47, 54),
                                    );
                                }
                            }
                        }

                        let mut checked = row.current;
                        let checkbox =
                            ui.add_enabled(!protected, egui::Checkbox::without_text(&mut checked));
                        if checkbox.changed() {
                            row.current = checked;
                        }
                        if protected {
                            checkbox.on_hover_text(
                                "Protected by the game's server. Steam will not accept changes.",
                            );
                        }

                        ui.vertical(|ui| {
                            ui.horizontal(|ui| {
                                let mut title = egui::RichText::new(&row.name).strong();
                                if modified {
                                    title = title.color(egui::Color32::from_rgb(120, 180, 250));
                                }
                                ui.label(title);

                                if protected {
                                    ui.label(
                                        egui::RichText::new("protected")
                                            .size(10.5)
                                            .color(egui::Color32::from_rgb(210, 150, 90)),
                                    );
                                }
                                if row.hidden {
                                    ui.label(egui::RichText::new("hidden").size(10.5).weak());
                                }
                                if row.original && row.unlock_time > 0 {
                                    ui.label(
                                        egui::RichText::new(format_unix_utc(row.unlock_time))
                                            .size(10.5)
                                            .weak(),
                                    );
                                }
                            });

                            if !row.description.is_empty() {
                                ui.label(egui::RichText::new(&row.description).size(11.5).weak());
                            }
                        });
                    });
                    ui.separator();
                }
            });
    }

    fn statistics_tab(&mut self, ui: &mut egui::Ui) {
        if self.stats.is_empty() {
            ui.centered_and_justified(|ui| {
                ui.label("This game reports no statistics.");
            });
            return;
        }

        ui.horizontal(|ui| {
            ui.checkbox(&mut self.allow_stat_edits, "Allow editing")
                .on_hover_text(
                    "Statistics often drive achievement progress and anti-cheat checks. \
                     Editing them is riskier than toggling achievements.",
                );
            if !self.allow_stat_edits {
                ui.label(egui::RichText::new("read-only").weak());
            }
        });
        ui.separator();

        let editable = self.allow_stat_edits;

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Grid::new("stats")
                    .num_columns(4)
                    .striped(true)
                    .spacing([12.0, 6.0])
                    .show(ui, |ui| {
                        ui.label(egui::RichText::new("Name").strong());
                        ui.label(egui::RichText::new("Value").strong());
                        ui.label(egui::RichText::new("Type").strong());
                        ui.label(egui::RichText::new("Flags").strong());
                        ui.end_row();

                        for row in self.stats.iter_mut() {
                            let protected = row.is_protected();

                            ui.label(&row.display_name).on_hover_text(&row.id);

                            let enabled = editable && !protected;
                            let response = ui.add_enabled(
                                enabled,
                                egui::TextEdit::singleline(&mut row.text).desired_width(110.0),
                            );
                            if response.changed() {
                                row.problem = None;
                            }

                            ui.label(match row.value {
                                StatValue::Integer { .. } => "integer",
                                StatValue::Float { .. } => "float",
                            });

                            ui.horizontal(|ui| {
                                if protected {
                                    ui.label(
                                        egui::RichText::new("protected")
                                            .color(egui::Color32::from_rgb(210, 150, 90)),
                                    );
                                }
                                if row.increment_only {
                                    ui.label(egui::RichText::new("increment only").weak());
                                }
                                if let Some(problem) = &row.problem {
                                    ui.colored_label(
                                        egui::Color32::from_rgb(232, 120, 120),
                                        problem,
                                    );
                                } else if row.is_modified() {
                                    ui.colored_label(
                                        egui::Color32::from_rgb(120, 180, 250),
                                        format!("was {}", row.original_text()),
                                    );
                                }
                            });
                            ui.end_row();
                        }
                    });
            });
    }
}

fn translate_result(result: i32) -> String {
    match result {
        1 => "ok".to_string(),
        2 => "generic error — this usually means you do not own the game".to_string(),
        8 => "invalid parameter".to_string(),
        // Steam has many EResult values; the number is still actionable.
        other => format!("EResult {other}"),
    }
}

/// Render a Unix timestamp as a UTC date.
///
/// Rendering in local time would need the system timezone database; UTC is
/// unambiguous and needs no dependency, so the label says so explicitly.
fn format_unix_utc(timestamp: u32) -> String {
    let seconds = timestamp as i64;
    let days = seconds.div_euclid(86_400);
    let time_of_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let (hour, minute) = (time_of_day / 3600, (time_of_day % 3600) / 60);
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02} UTC")
}

/// Days since the Unix epoch to a civil date (Howard Hinnant's algorithm).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    // Shift the era so that it starts on 0000-03-01.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_known_timestamps() {
        assert_eq!(format_unix_utc(0), "1970-01-01 00:00 UTC");
        // 2001-09-09T01:46:40Z, the classic billennium timestamp.
        assert_eq!(format_unix_utc(1_000_000_000), "2001-09-09 01:46 UTC");
        // A leap day.
        assert_eq!(format_unix_utc(1_709_164_800), "2024-02-29 00:00 UTC");
    }

    #[test]
    fn civil_conversion_round_trips_year_boundaries() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
        assert_eq!(civil_from_days(365), (1971, 1, 1));
    }
}
