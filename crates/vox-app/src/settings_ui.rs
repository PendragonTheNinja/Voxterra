//! The in-game settings menu (Milestone 09, amendment A3).
//!
//! Opened with ESC. Exists because every visual decision in M09 cost a
//! recompile-and-squint cycle — change a constant, rebuild, fly out, judge a
//! screenshot. Live sliders turn that into seconds, which is the tool the
//! remaining tuning work needs.
//!
//! The app owns the egui context because egui needs winit events; the renderer
//! only draws the tessellated output (`vox_render::UiFrame`).

use vox_core::settings::{ranges, Settings};

/// Owns the egui context and winit bridge, and tracks whether the menu is open.
pub struct SettingsUi {
    ctx: egui::Context,
    state: egui_winit::State,
    /// Menu visible. While open the cursor is released and mouse-look is off.
    pub open: bool,
    /// Which screen is showing (pause vs options), like a console pause menu.
    screen: Screen,
    /// Set when "Save and Quit" is pressed; the app saves and exits.
    pub quit_requested: bool,
    /// Live-updated readout shown in the menu, so numbers can be judged
    /// against what the sliders are doing.
    pub status: String,
}

impl SettingsUi {
    pub fn new(window: &winit::window::Window) -> Self {
        let ctx = egui::Context::default();
        let state = egui_winit::State::new(
            ctx.clone(),
            egui::ViewportId::ROOT,
            window,
            Some(window.scale_factor() as f32),
            None,
            None,
        );
        Self {
            ctx,
            state,
            open: false,
            screen: Screen::Pause,
            quit_requested: false,
            status: String::new(),
        }
    }

    /// ESC pressed while the menu is open: step back one screen, or close.
    /// Returns true if the menu is now closed (caller recaptures the cursor).
    pub fn on_escape(&mut self) -> bool {
        match self.screen {
            Screen::Options => {
                self.screen = Screen::Pause;
                false
            }
            Screen::Pause => {
                self.open = false;
                true
            }
        }
    }

    /// Feed a window event to egui. Returns true if egui consumed it, in which
    /// case the game must ignore it — otherwise clicking a slider would also
    /// swing the camera or break a block.
    pub fn on_window_event(
        &mut self,
        window: &winit::window::Window,
        event: &winit::event::WindowEvent,
    ) -> bool {
        if !self.open {
            return false;
        }
        self.state.on_window_event(window, event).consumed
    }

    /// Build this frame's UI and return the tessellated output.
    ///
    /// Runs even when the menu is closed, because the crosshair is drawn here
    /// too — see [`Self::crosshair`] for why that is worth a per-frame egui
    /// pass. While closed the pass paints two circles and nothing else, and
    /// egui receives no events (`on_window_event` returns early), so there is
    /// no interaction cost.
    pub fn run(
        &mut self,
        window: &winit::window::Window,
        settings: &mut Settings,
    ) -> Option<UiOutput> {
        let raw_input = self.state.take_egui_input(window);
        let status = self.status.clone();
        let screen = &mut self.screen;
        let mut close = false;
        let mut quit = false;

        let menu_open = self.open;
        let full = self.ctx.run(raw_input, |ctx| {
            if !menu_open {
                Self::crosshair(ctx);
                return;
            }
            // Dim the world behind the menu, so the UI reads as a layer over a
            // paused game rather than a panel floating in the scene.
            egui::Area::new(egui::Id::new("menu_dim"))
                .interactable(false)
                .fixed_pos(egui::Pos2::ZERO)
                .show(ctx, |ui| {
                    ui.painter().rect_filled(
                        ctx.screen_rect(),
                        0.0,
                        egui::Color32::from_black_alpha(150),
                    );
                });

            // Centred, like a pause screen should be — not parked in a corner.
            egui::Area::new(egui::Id::new("menu_root"))
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .show(ctx, |ui| {
                    egui::Frame::window(&ctx.style()).show(ui, |ui| {
                        ui.set_width(MENU_WIDTH);
                        ui.vertical_centered(|ui| match screen {
                            Screen::Pause => {
                                Self::pause_screen(ui, screen, &mut close, &mut quit);
                            }
                            Screen::Options => {
                                Self::options_screen(ui, settings, &status, screen);
                            }
                        });
                    });
                });
        });

        if close {
            self.open = false;
            // Reopening lands on the pause screen, not wherever we left off.
            self.screen = Screen::Pause;
        }
        self.quit_requested |= quit;
        // Text entry and drags can leave values out of range; repair before
        // anything downstream sees them.
        settings.sanitize();

        self.state
            .handle_platform_output(window, full.platform_output.clone());
        let primitives = self.ctx.tessellate(full.shapes, full.pixels_per_point);
        Some(UiOutput {
            primitives,
            textures_set: full.textures_delta.set,
            textures_free: full.textures_delta.free,
            pixels_per_point: full.pixels_per_point,
        })
    }

    /// A small dot at the exact centre of the screen, marking where the
    /// raycast is aimed.
    ///
    /// Drawn through egui rather than a dedicated wgpu pass: egui is already
    /// wired up (A3) and this costs two tessellated circles, where a
    /// screen-space quad would need its own shader, pipeline, vertex buffer
    /// and resize handling for the same two circles.
    ///
    /// Dark disc under a light one, rather than a single dot, so it stays
    /// readable against both bright sky and dark stone without needing a
    /// blend mode or an inverting shader.
    fn crosshair(ctx: &egui::Context) {
        let centre = ctx.screen_rect().center();
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("crosshair"),
        ));
        painter.circle_filled(
            centre,
            CROSSHAIR_RADIUS + 1.0,
            egui::Color32::from_black_alpha(150),
        );
        painter.circle_filled(
            centre,
            CROSSHAIR_RADIUS,
            egui::Color32::from_white_alpha(230),
        );
    }

    /// The pause screen: title and a stack of equally sized buttons.
    fn pause_screen(ui: &mut egui::Ui, screen: &mut Screen, close: &mut bool, quit: &mut bool) {
        ui.add_space(6.0);
        ui.label(egui::RichText::new("Voxterra").size(30.0).strong());
        ui.label(egui::RichText::new("Paused").weak());
        ui.add_space(14.0);

        if Self::menu_button(ui, "Back to Game").clicked() {
            *close = true;
        }
        ui.add_space(BUTTON_GAP);
        if Self::menu_button(ui, "Options…").clicked() {
            *screen = Screen::Options;
        }
        ui.add_space(BUTTON_GAP);
        if Self::menu_button(ui, "Save and Quit").clicked() {
            *quit = true;
        }
        ui.add_space(10.0);
        ui.label(egui::RichText::new("ESC to resume").small().weak());
        ui.add_space(4.0);
    }

    /// The options screen: grouped settings, then a Done button back to pause.
    fn options_screen(
        ui: &mut egui::Ui,
        settings: &mut Settings,
        status: &str,
        screen: &mut Screen,
    ) {
        ui.add_space(4.0);
        ui.label(egui::RichText::new("Options").size(24.0).strong());
        ui.add_space(8.0);

        egui::ScrollArea::vertical()
            .max_height(420.0)
            .show(ui, |ui| {
                ui.vertical(|ui| {
                    Self::section(ui, "View", |ui| {
                        ui.add(
                            egui::Slider::new(
                                &mut settings.load_radius,
                                ranges::LOAD_RADIUS.0..=ranges::LOAD_RADIUS.1,
                            )
                            .text("full-res radius (chunks)"),
                        );
                        ui.add(
                            egui::Slider::new(
                                &mut settings.lod_extra_levels,
                                ranges::LOD_EXTRA_LEVELS.0..=ranges::LOD_EXTRA_LEVELS.1,
                            )
                            .text("extra LOD levels"),
                        );
                        ui.label(
                            egui::RichText::new(
                                "Each extra level doubles view distance for about the same cost.",
                            )
                            .small()
                            .weak(),
                        );
                        ui.checkbox(&mut settings.lod_enabled, "LOD terrain");
                        ui.add(
                            egui::Slider::new(
                                &mut settings.fov_degrees,
                                ranges::FOV_DEGREES.0..=ranges::FOV_DEGREES.1,
                            )
                            .text("field of view"),
                        );
                        ui.label(
                            egui::RichText::new(
                                "Radius and LOD changes re-stream the world; expect a pause.",
                            )
                            .small()
                            .weak(),
                        );
                    });

                    Self::section(ui, "Fog", |ui| {
                        ui.checkbox(&mut settings.fog_enabled, "fog");
                        let on = settings.fog_enabled;
                        ui.add_enabled_ui(on, |ui| {
                            ui.checkbox(&mut settings.fog_auto, "scale fog to view distance");
                            ui.label(
                                egui::RichText::new(
                                    "Fog distances follow the LOD horizon, so changing LOD \
                                     levels doesn't need fog re-tuned.",
                                )
                                .small()
                                .weak(),
                            );
                            if settings.fog_auto {
                                ui.add(
                                    egui::Slider::new(
                                        &mut settings.fog_start_frac,
                                        ranges::FOG_FRAC.0..=ranges::FOG_FRAC.1,
                                    )
                                    .text("start (× horizon)"),
                                );
                                ui.add(
                                    egui::Slider::new(
                                        &mut settings.fog_end_frac,
                                        ranges::FOG_FRAC.0..=ranges::FOG_FRAC.1,
                                    )
                                    .text("end (× horizon)"),
                                );
                            } else {
                                ui.add(
                                    egui::Slider::new(
                                        &mut settings.fog_start,
                                        ranges::FOG_START.0..=ranges::FOG_START.1,
                                    )
                                    .text("start (blocks)"),
                                );
                                ui.add(
                                    egui::Slider::new(
                                        &mut settings.fog_end,
                                        ranges::FOG_END.0..=ranges::FOG_END.1,
                                    )
                                    .text("end (blocks)"),
                                );
                            }
                            ui.add(
                                egui::Slider::new(
                                    &mut settings.fog_strength,
                                    ranges::FOG_STRENGTH.0..=ranges::FOG_STRENGTH.1,
                                )
                                .text("strength"),
                            );
                        });
                    });

                    Self::section(ui, "LOD geomorph", |ui| {
                        ui.label(
                            egui::RichText::new(
                                "How far before a LOD ring boundary terrain starts \
                                 morphing into the next coarser level. 0 = off.",
                            )
                            .small()
                            .weak(),
                        );
                        ui.add(
                            egui::Slider::new(
                                &mut settings.lod_morph_band,
                                ranges::LOD_MORPH_BAND.0..=ranges::LOD_MORPH_BAND.1,
                            )
                            .text("morph band (blocks)"),
                        );
                    });

                    Self::section(ui, "Time & sky", |ui| {
                        ui.checkbox(&mut settings.time_paused, "pause day/night");
                        ui.add(
                            egui::Slider::new(&mut settings.time_of_day, 0.0..=1.0)
                                .text("time of day")
                                .custom_formatter(|v, _| {
                                    let mins = (v * 24.0 * 60.0) as i32;
                                    format!("{:02}:{:02}", mins / 60, mins % 60)
                                }),
                        );
                        ui.add(
                            egui::Slider::new(
                                &mut settings.day_length_secs,
                                ranges::DAY_LENGTH_SECS.0..=ranges::DAY_LENGTH_SECS.1,
                            )
                            .logarithmic(true)
                            .text("day length (real s)"),
                        );
                        ui.add(
                            egui::Slider::new(
                                &mut settings.star_intensity,
                                ranges::STAR_INTENSITY.0..=ranges::STAR_INTENSITY.1,
                            )
                            .text("stars"),
                        );
                    });

                    Self::section(ui, "Debug", |ui| {
                        ui.checkbox(&mut settings.show_telemetry, "log telemetry");
                        ui.label(egui::RichText::new(status).monospace().small().weak());
                    });
                });
            });

        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if ui
                .add_sized(
                    [MENU_WIDTH * 0.45, BUTTON_HEIGHT],
                    egui::Button::new("Defaults"),
                )
                .clicked()
            {
                *settings = Settings::default();
            }
            if ui
                .add_sized(
                    [MENU_WIDTH * 0.45, BUTTON_HEIGHT],
                    egui::Button::new("Done"),
                )
                .clicked()
            {
                *screen = Screen::Pause;
            }
        });
        ui.add_space(4.0);
    }

    /// A titled group, so options read as sections rather than one long list.
    fn section(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui)) {
        ui.add_space(6.0);
        ui.label(egui::RichText::new(title).strong());
        ui.separator();
        add(ui);
        ui.add_space(4.0);
    }

    /// Uniformly sized menu button, so the stack lines up.
    fn menu_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
        ui.add_sized(
            [MENU_WIDTH * 0.8, BUTTON_HEIGHT],
            egui::Button::new(egui::RichText::new(text).size(16.0)),
        )
    }
}

/// Which screen the menu is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Pause,
    Options,
}

/// Crosshair dot radius in logical pixels. Small enough not to obscure the
/// block being aimed at, large enough to find on a busy skyline.
const CROSSHAIR_RADIUS: f32 = 2.0;
const MENU_WIDTH: f32 = 380.0;
const BUTTON_HEIGHT: f32 = 34.0;
const BUTTON_GAP: f32 = 8.0;

/// Owned egui output for one frame; borrowed into `vox_render::UiFrame`.
pub struct UiOutput {
    pub primitives: Vec<egui::ClippedPrimitive>,
    pub textures_set: Vec<(egui::TextureId, egui::epaint::ImageDelta)>,
    pub textures_free: Vec<egui::TextureId>,
    pub pixels_per_point: f32,
}
