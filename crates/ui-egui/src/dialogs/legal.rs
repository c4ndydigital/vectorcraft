//! In-app modals for Impressum and Datenschutz.

use crate::VectorcraftApp;
use crate::theme;

/// Show Impressum modal window if `app.ui.impressum` is active.
pub fn show_impressum(app: &mut VectorcraftApp, ctx: &egui::Context) {
    if !app.ui.impressum {
        return;
    }
    let mut open = true;
    let mut close = false;
    egui::Window::new(tl!("Impressum"))
        .collapsible(false)
        .resizable(true)
        .open(&mut open)
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
        .default_size(egui::vec2(440.0, 360.0))
        .frame(egui::Frame::window(&ctx.global_style()).inner_margin(egui::Margin::same(18)))
        .show(ctx, |ui| {
            ui.vertical(|ui| {
                ui.label(egui::RichText::new("C4NDY Digital UG (haftungsbeschränkt)").font(theme::semibold(16.0)));
                ui.add_space(4.0);
                ui.label("Hauptstraße 34\n89284 Pfaffenhofen a.d. Roth");
                ui.add_space(10.0);
                ui.label(egui::RichText::new("Vertretungsberechtigt:").font(theme::semibold(13.0)));
                ui.label("Adrian Jehne (Geschäftsführer)");
                ui.add_space(10.0);
                ui.label(egui::RichText::new("Kontakt:").font(theme::semibold(13.0)));
                ui.label("Telefon: +49 (0) 7302 – 7090979\nE-Mail: hello@ndy.onl");
                ui.add_space(10.0);
                ui.label(egui::RichText::new("Registereintrag:").font(theme::semibold(13.0)));
                ui.label("Amtsgericht Memmingen, HRB 15637");
                ui.add_space(14.0);
                ui.separator();
                ui.add_space(8.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Schließen").clicked() {
                        close = true;
                    }
                });
            });
        });
    app.ui.impressum = open && !close;
}

/// Show Datenschutz modal window if `app.ui.datenschutz` is active.
pub fn show_datenschutz(app: &mut VectorcraftApp, ctx: &egui::Context) {
    if !app.ui.datenschutz {
        return;
    }
    let mut open = true;
    let mut close = false;
    egui::Window::new(tl!("Datenschutzerklärung"))
        .collapsible(false)
        .resizable(true)
        .open(&mut open)
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
        .default_size(egui::vec2(520.0, 480.0))
        .frame(egui::Frame::window(&ctx.global_style()).inner_margin(egui::Margin::same(18)))
        .show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new("1. Verantwortlicher").font(theme::semibold(14.0)));
                    ui.label("C4NDY Digital UG (haftungsbeschränkt)\nHauptstraße 34, 89284 Pfaffenhofen a.d. Roth\nE-Mail: hello@ndy.onl");
                    ui.add_space(10.0);

                    ui.label(egui::RichText::new("2. Hosting auf eigener Infrastruktur").font(theme::semibold(14.0)));
                    ui.label("Diese Web-Applikation und verbundene Dienste werden auf unserer eigenen Hardware in Pfaffenhofen a.d. Roth betrieben. Maximale Datensouveränität.");
                    ui.add_space(10.0);

                    ui.label(egui::RichText::new("3. Echtzeit-Kollaboration (Flüchtige Räume)").font(theme::semibold(14.0)));
                    ui.label("Zeichnungsdaten, CRDT-Deltas und Mauszeiger werden flüchtig im Arbeitsspeicher (RAM) synchronisiert. Nach dem Verlassen aller Teilnehmer werden Räume automatisch gelöscht. Es erfolgt keine dauerhafte Speicherung auf Server-Festplatten.");
                    ui.add_space(10.0);

                    ui.label(egui::RichText::new("4. Keine Tracking-Cookies").font(theme::semibold(14.0)));
                    ui.label("Wir setzen keine Cookies ein. Der lokale Speicher (LocalStorage / IndexedDB) wird ausschließlich technisch notwendig für die Wiederherstellung von Entwürfen genutzt (§ 25 Abs. 2 TDDDG).");
                    ui.add_space(10.0);

                    ui.label(egui::RichText::new("5. Web-Analyse mit Umami").font(theme::semibold(14.0)));
                    ui.label("Wir nutzen selbstgehostetes Umami zur cookielosen, aggregierten Zugriffsstatistik ohne Speicherung personenbezogener Daten (Art. 6 Abs. 1 lit. f DSGVO).");
                    ui.add_space(14.0);
                    ui.separator();
                    ui.add_space(8.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Schließen").clicked() {
                            close = true;
                        }
                    });
                });
            });
        });
    app.ui.datenschutz = open && !close;
}

/// Show room password configuration modal window if `app.ui.room_password` is active.
pub fn show_room_password(app: &mut VectorcraftApp, ctx: &egui::Context) {
    if !app.ui.room_password {
        return;
    }
    let mut open = true;
    let mut close = false;
    egui::Window::new(tl!("Raum-Passwort & Zugriffsschutz"))
        .collapsible(false)
        .resizable(false)
        .open(&mut open)
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
        .default_size(egui::vec2(420.0, 240.0))
        .frame(egui::Frame::window(&ctx.global_style()).inner_margin(egui::Margin::same(18)))
        .show(ctx, |ui| {
            ui.vertical(|ui| {
                ui.label(egui::RichText::new("Kollaborationsraum schützen").font(theme::semibold(16.0)));
                ui.add_space(4.0);
                ui.label(
                    "Lege eine PIN oder ein Passwort für diesen Raum fest. \
                     Neue Teilnehmer müssen dieses Passwort eingeben oder über den geschützten Freigabelink beitreten.",
                );
                ui.add_space(12.0);

                let is_protected = app.collab_room_protected;
                let t = theme::Tokens::get(ui.ctx());

                ui.horizontal(|ui| {
                    if is_protected {
                        ui.label(egui::RichText::new("Status:").font(theme::semibold(13.0)));
                        ui.label(egui::RichText::new("🔒 Passwortgeschützt").color(egui::Color32::from_rgb(10, 207, 131)).font(theme::semibold(13.0)));
                    } else {
                        ui.label(egui::RichText::new("Status:").font(theme::semibold(13.0)));
                        ui.label(egui::RichText::new("🔓 Öffentlich (kein Passwort)").color(t.text_dim));
                    }
                });
                ui.add_space(8.0);

                ui.horizontal(|ui| {
                    ui.label("Passwort / PIN:");
                    let edit = egui::TextEdit::singleline(&mut app.collab_password_input)
                        .password(true)
                        .hint_text("z. B. GeheimesPasswort123")
                        .desired_width(180.0);
                    ui.add(edit);
                });

                ui.add_space(14.0);
                ui.separator();
                ui.add_space(8.0);

                ui.horizontal(|ui| {
                    if ui.add(egui::Button::new(if is_protected { "Passwort aktualisieren" } else { "Passwort aktivieren" })).clicked() {
                        let trimmed = app.collab_password_input.trim().to_string();
                        if !trimmed.is_empty() {
                            app.set_collab_password(Some(trimmed));
                            close = true;
                        }
                    }

                    if is_protected && ui.add(egui::Button::new(egui::RichText::new("Schutz aufheben").color(egui::Color32::from_rgb(242, 78, 30)))).clicked() {
                        app.set_collab_password(None);
                        app.collab_password_input.clear();
                        close = true;
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Schließen").clicked() {
                            close = true;
                        }
                    });
                });
            });
        });
    app.ui.room_password = open && !close;
}

