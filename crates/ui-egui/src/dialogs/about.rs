//! Help → About VectorCraft.

use crate::VectorcraftApp;
use crate::icons;
use crate::theme;

/// The About window, while `UiState::about` is set.
pub(super) fn show(app: &mut VectorcraftApp, ctx: &egui::Context) {
    if !app.ui.about {
        return;
    }
    let mut open = true;
    egui::Window::new(tl!("About VectorCraft"))
        .collapsible(false)
        .resizable(false)
        .open(&mut open)
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
        .frame(egui::Frame::window(&ctx.global_style()).inner_margin(egui::Margin::same(18)))
        .show(ctx, |ui| {
            // Tabs: About · Contributors · Models · KI & MCP (the credits are compiled in, see `credits`).
            let tab_id = egui::Id::new("about_tab");
            let mut tab = ui.data_mut(|d| d.get_temp::<u8>(tab_id)).unwrap_or(0);
            ui.horizontal(|ui| {
                for (i, label) in [tl!("About"), tl!("Contributors"), tl!("Models"), tl!("KI & MCP Connect")].into_iter().enumerate() {
                    let i = i as u8;
                    if ui.selectable_label(tab == i, label).clicked() {
                        tab = i;
                    }
                }
            });
            ui.data_mut(|d| d.insert_temp(tab_id, tab));
            ui.separator();
            match tab {
                1 => {
                    ui.set_width(660.0);
                    crate::credits::contributors_ui(app, ui);
                }
                2 => {
                    ui.set_width(660.0);
                    crate::credits::models_ui(ui);
                }
                3 => {
                    ui.set_width(520.0);
                    mcp_connect_tab(app, ui);
                }
                _ => about_tab(app, ui),
            }
        });
    app.ui.about = open;
}

/// The About tab: mark, version, community links, licences and the GPU in use.
fn about_tab(app: &mut VectorcraftApp, ui: &mut egui::Ui) {
    ui.set_width(380.0);
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(egui::vec2(44.0, 44.0), egui::Sense::hover());
        crate::brand::paint_mark(ui, r);
        ui.vertical(|ui| {
            ui.label(egui::RichText::new("VectorCraft").font(theme::semibold(22.0)));
            ui.label(crate::i18n::fmt(
                tl!("Version {version} — open-source vector illustration in pure Rust."),
                &[("version", env!("CARGO_PKG_VERSION"))],
            ));
        });
    });
    ui.add_space(8.0);
    ui.label(
        egui::RichText::new("Ein modernes Vector- und Kollaborations-Studio von C4NDY Digital.")
            .font(theme::semibold(13.0))
            .color(theme::Tokens::get(ui.ctx()).accent),
    );
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(
            "Weiterentwickelter, optimierter Fork von VectorCraft mit Echtzeit-Multiplayer, \
             Live-MCP-Anbindung für KI-Agenten, DSGVO-konformem Hosting und PDF-Image-Synchronisation.",
        )
        .size(12.0),
    );
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        let t = theme::Tokens::get(ui.ctx());
        icons::icon(ui, "git-branch", 16.0, t.icon);
        let r = ui.add(egui::Button::new(egui::RichText::new("C4NDY Digital auf GitHub").color(t.accent).size(12.5)).frame(false));
        if r.clicked() {
            app.open_link("help.github");
        }
    });
    ui.add_space(8.0);
    ui.label(
        egui::RichText::new(
            "Forked from VectorCraft (Storytold / ArtCraft Team). Lizenz: MIT OR Apache-2.0. Fonts: Source Sans 3, Inter, JetBrains Mono (OFL). Icons: Lucide (ISC) + VectorCraft."
        )
        .size(11.0),
    );
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        let t = theme::Tokens::get(ui.ctx());
        if ui.add(egui::Button::new(egui::RichText::new("Impressum").color(t.accent).size(11.0)).frame(false)).clicked() {
            app.ui.impressum = true;
        }
        ui.label(egui::RichText::new("·").color(t.text_dim).size(11.0));
        if ui.add(egui::Button::new(egui::RichText::new("Datenschutz").color(t.accent).size(11.0)).frame(false)).clicked() {
            app.ui.datenschutz = true;
        }
    });
    // The GPU the window renders with, for bug reports (Preferences › Performance picks it).
    if let Some(adapter) = &app.graphics_adapter {
        ui.add_space(6.0);
        ui.label(egui::RichText::new(crate::i18n::fmt(tl!("Graphics: {adapter}"), &[("adapter", adapter.as_str())])).size(11.0));
    }
}

/// The KI & MCP Connect tab: Instructions for Claude, Cursor, and custom MCP connectors.
fn mcp_connect_tab(app: &mut VectorcraftApp, ui: &mut egui::Ui) {
    let t = theme::Tokens::get(ui.ctx());
    let room_id = &app.collab_room_id;
    let sse_url = format!("https://vector.c4ndy.com/mcp/sse?room={room_id}");
    let rpc_url = format!("https://vector.c4ndy.com/mcp?room={room_id}");

    ui.vertical(|ui| {
        ui.label(egui::RichText::new("VectorCraft mit KI verbinden (Claude, Cursor & MCP)").font(theme::semibold(15.0)));
        ui.add_space(4.0);
        ui.label(
            "Jeder VectorCraft-Raum ist ein vollständiger Live-MCP-Server. \
             Damit eine KI (wie Claude) auf deiner Zeichenfläche zeichnen kann, muss VectorCraft \
             als MCP-Connector hinzugefügt werden (ein reiner Weblink reicht für Schreibzugriff nicht aus).",
        );
        ui.add_space(10.0);

        ui.label(egui::RichText::new("1. Claude Web / Claude Desktop").font(theme::semibold(13.0)));
        ui.label("Gehe in Claude zu: Einstellungen → Connectors → Benutzerdefinierten Connector hinzufügen.");
        ui.add_space(4.0);

        ui.horizontal(|ui| {
            ui.label("Connector-URL (SSE):");
            if ui.button("📋 SSE-URL kopieren").clicked() {
                ui.ctx().copy_text(sse_url.clone());
            }
        });
        ui.label(egui::RichText::new(&sse_url).color(t.accent).size(11.5));

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label("Direkte RPC-URL:");
            if ui.button("📋 RPC-URL kopieren").clicked() {
                ui.ctx().copy_text(rpc_url.clone());
            }
        });
        ui.label(egui::RichText::new(&rpc_url).color(t.text_dim).size(11.5));

        ui.add_space(10.0);
        ui.label(egui::RichText::new("2. Claude Desktop Config (JSON)").font(theme::semibold(13.0)));
        let desktop_cfg = format!(
            "{{\n  \"mcpServers\": {{\n    \"vectorcraft\": {{\n      \"command\": \"npx\",\n      \"args\": [\"-y\", \"@modelcontextprotocol/server-sse\", \"{sse_url}\"]\n    }}\n  }}\n}}"
        );
        if ui.button("📋 claude_desktop_config.json kopieren").clicked() {
            ui.ctx().copy_text(desktop_cfg.clone());
        }
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new("Sobald die Verbindung aktiv ist, kann Claude mit Befehlen wie „Zeichne einen roten Kreis“ direkt live in deinem Raum arbeiten!")
                .font(theme::semibold(11.5))
                .color(egui::Color32::from_rgb(10, 207, 131)),
        );
    });
}

