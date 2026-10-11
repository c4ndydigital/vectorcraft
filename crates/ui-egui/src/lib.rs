//! VectorCraft's egui frontend: an Illustrator-style UI over `vectorcraft-engine`.
//!
//! The UI is thin: every action goes through [`VectorcraftApp::run`], which dispatches UI commands
//! (view/window) here and everything else to the engine. The same entry point serves menus,
//! shortcuts, the ⌘K palette and the control channel ([`control`]).
#![forbid(unsafe_code)]

/// Translate a UI string literal into the language the UI is drawn in (see [`i18n`]).
#[macro_export]
macro_rules! tl {
    ($s:expr) => {
        $crate::i18n::t($s)
    };
}

pub mod background;
mod brand;
pub mod canvas;
pub mod chrome;
mod clipboard_probe;
pub mod community;
pub mod control;
pub mod credits;
pub mod cursors;
pub mod dialogs;
pub mod dock;
pub mod find_font;
pub mod floating;
pub mod font_menu;
pub mod graphics;
pub mod i18n;
pub mod icon_data;
pub mod icons;
pub mod io;
pub mod menus;
pub mod native_menu;
pub mod palette;
pub mod panels;
pub mod place;
pub mod prefs_dialog;
pub mod print;
pub mod recovery;
pub mod render_worker;
mod scrub;
pub mod shortcut_editor;
pub mod shortcuts;
pub mod state;
pub mod sysclip;
pub mod theme;
pub mod titlebar;
pub mod toolbar;
mod ui_fonts;
pub mod unsaved;
pub mod widgets;
pub mod workspaces;

#[cfg(test)]
mod tests_adjust;
#[cfg(test)]
mod tests_aisave;
#[cfg(test)]
mod tests_background;
#[cfg(test)]
mod tests_clipboard;
#[cfg(test)]
mod tests_contextmenu;
#[cfg(test)]
mod tests_cut;
#[cfg(test)]
mod tests_distortkeys;
#[cfg(test)]
mod tests_docsetup;
#[cfg(test)]
mod tests_font_menu;
#[cfg(test)]
mod tests_fonts;
#[cfg(test)]
mod tests_home;
#[cfg(test)]
mod tests_labels;
#[cfg(test)]
mod tests_nativemenu;
#[cfg(test)]
mod tests_nativeoptions;
#[cfg(test)]
mod tests_numfields;
#[cfg(test)]
mod tests_overprint;
#[cfg(test)]
mod tests_paintchips;
#[cfg(test)]
mod tests_pastechords;
#[cfg(test)]
mod tests_pathtype;
#[cfg(test)]
mod tests_pdfoutput;
#[cfg(test)]
mod tests_place;
#[cfg(test)]
mod tests_plugins;
#[cfg(test)]
mod tests_printps;
#[cfg(test)]
mod tests_printtiling;
#[cfg(test)]
mod tests_puppetwarp;
#[cfg(test)]
mod tests_recolor;
#[cfg(test)]
mod tests_recovery;
#[cfg(test)]
mod tests_removeanchors;
#[cfg(test)]
mod tests_save;
#[cfg(test)]
mod tests_saveext;
#[cfg(test)]
mod tests_screenmode;
#[cfg(test)]
mod tests_scrub;
#[cfg(test)]
mod tests_selectall;
#[cfg(test)]
mod tests_slices;
#[cfg(test)]
mod tests_svg;
#[cfg(test)]
mod tests_svgsave;
#[cfg(test)]
mod tests_synthetic;
#[cfg(test)]
mod tests_sysclip;
#[cfg(test)]
mod tests_sysclip_emf;
#[cfg(test)]
mod tests_sysclip_probe;
#[cfg(test)]
mod tests_transparencygrid;
#[cfg(test)]
mod tests_widthtool;

use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use vectorcraft_engine::cmd::fileio;
use vectorcraft_engine::{Session, ViewInfo};

pub use control::{ControlRequest, ControlResponse};
pub use serde;
pub use serde_json;
pub use state::{UiState, View};
pub use sysclip::{ClipboardProbeFactory, SystemClipboard};

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct PeerPresence {
    pub client_id: u64,
    pub name: String,
    pub color: [u8; 4],
    pub cursor: Option<vectorcraft_geom::Point>,
    pub last_seen: f64,
}

#[derive(Clone, Debug)]
pub enum CollabIncoming {
    Update(Vec<u8>),
    Presence(PeerPresence),
}

#[derive(Clone, Debug)]
pub enum CollabOutgoing {
    Update(Vec<u8>),
    Presence(PeerPresence),
}

/// What a file dialog shows: a suggested file name (save dialogs), the folder to start in and the
/// file-type filters.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FilePick {
    pub name: String,
    pub folder: Option<String>,
    /// `(label, extensions without the dot)`, the default type first; empty = every file.
    pub filters: Vec<(&'static str, &'static [&'static str])>,
}

impl FilePick {
    /// A save dialog suggesting `name`.
    pub fn named(name: &str) -> Self {
        Self { name: name.to_string(), ..Default::default() }
    }
}

pub type PickOpen = Box<dyn FnMut(&FilePick) -> Option<String>>;
pub type PickSave = Box<dyn FnMut(&FilePick) -> Option<String>>;
pub type ReadFn = Box<dyn Fn(&str) -> Result<Vec<u8>, String>>;
pub type WriteFn = Box<dyn FnMut(&str, &[u8]) -> Result<(), String>>;
pub type Inbox = Arc<Mutex<Vec<(String, Vec<u8>)>>>;
pub type DownloadFn = Box<dyn FnMut(&str, &[u8])>;

/// Opens a URL in the system browser.
pub type OpenUrlFn = Box<dyn FnMut(&str)>;

/// Shows a file in the system file manager, or opens it in its app.
pub type RevealFn = Box<dyn FnMut(&str) -> Result<(), String>>;

/// Platform services injected by the host app (desktop or web).
#[derive(Default)]
pub struct Services {
    /// Show an open dialog; returns a path.
    pub pick_open: Option<PickOpen>,
    /// Show a save dialog (suggested name, folder, file types); returns a path.
    pub pick_save: Option<PickSave>,
    pub read: Option<ReadFn>,
    pub write: Option<WriteFn>,
    /// Files that arrived asynchronously (web open / drops): (name, bytes).
    pub inbox: Option<Inbox>,
    /// Web: trigger a browser download instead of writing a path.
    pub download: Option<DownloadFn>,
    /// Web: start an async open (bytes arrive via `inbox`).
    pub open_async: Option<Box<dyn FnMut()>>,
    /// Read the system clipboard's text (desktop). Without it, pasted text only arrives with
    /// egui's Paste event (web, and keyboard paste everywhere).
    pub clipboard_read: Option<Box<dyn FnMut() -> Option<String>>>,
    /// Open a URL in the system browser (desktop). Without it, egui opens it (a new tab on the web).
    pub open_url: Option<OpenUrlFn>,
    /// Show an open dialog for several files (File → Place…); returns their paths (none: cancelled).
    pub pick_open_multi: Option<Box<dyn FnMut() -> Vec<String>>>,
    /// Web: start an async pick of files to place (they arrive via `place_inbox`).
    pub place_async: Option<Box<dyn FnMut()>>,
    /// Files that arrived asynchronously to be placed (web: picked for Place, or dropped on the
    /// canvas).
    pub place_inbox: Option<place::PlaceInbox>,
    /// The system clipboard with all its formats (desktop): Copy offers text, SVG, PDF and PNG,
    /// Paste takes SVG, PDF, text and bitmaps from other apps. Without it, SVG text only (through
    /// egui and `clipboard_read`).
    pub system_clipboard: Option<Box<dyn SystemClipboard>>,
    /// A second system-clipboard handle, for checking whether Paste has something to take on a
    /// background thread (Linux: an X11 clipboard owner that never answers would freeze the UI).
    /// Taken on the first frame. Without it the check runs on the UI thread.
    pub clipboard_probe: Option<ClipboardProbeFactory>,
    /// File → Show in Folder: select a file in the system file manager (desktop).
    pub reveal: Option<RevealFn>,
    /// Write a file from any thread (desktop): lets Background Save and Export write off the UI
    /// thread ([`background`]). Without it they run at once.
    pub write_shared: Option<background::SharedWriteFn>,
    /// Open a file (or a folder) in the system's default app for it (desktop: Edit Original, Show
    /// Package). Without it (the web) those answer with an error.
    pub open_file: Option<RevealFn>,
    /// Show a folder picker; returns its path (desktop: Relink to Folder, Package).
    pub pick_folder: Option<Box<dyn FnMut() -> Option<String>>>,
    /// Where Data Recovery keeps its copies when not in a folder (the web's browser storage;
    /// tests): installed into the session ([`vectorcraft_engine::cmd::recovery`]).
    pub recovery_store: Option<std::sync::Arc<dyn vectorcraft_engine::cmd::recovery::RecoveryStore>>,
    /// File → Print: the system's printers and print queue (desktop), the browser's print dialog
    /// (web). Without it Print saves the job as a PDF.
    pub print: Option<Box<dyn print::PrintService>>,
    /// The macOS menu bar, when the desktop app installed one: the in-window menus are hidden then.
    pub native_menu: Option<native_menu::NativeMenu>,
    /// Multiplayer collaboration incoming queue (remote updates and presence).
    pub collab_inbox: Option<std::sync::Arc<std::sync::Mutex<Vec<CollabIncoming>>>>,
    /// Multiplayer collaboration outgoing queue (local updates and presence to broadcast).
    pub collab_outbox: Option<std::sync::Arc<std::sync::Mutex<Vec<CollabOutgoing>>>>,
}

/// Cached canvas raster.
pub struct CanvasCache {
    pub renderer: vectorcraft_render::Renderer,
    pub texture: Option<egui::TextureHandle>,
    pub key: Option<CacheKey>,
    pub last_ms: f64,
    pub worker: Option<render_worker::Worker>,
    pub worker_started: bool,
    /// The slices as laid out for (document uid, revision): the canvas draws them every frame.
    pub slices: Option<SliceCache>,
    /// The print tiling's pages for (document uid, revision) (View → Show Print Tiling).
    pub print_tiling: Option<PrintTilingCache>,
    /// [`VectorcraftApp::selection_box`] for (document uid, revision, Use Preview Bounds).
    pub selection_box: Option<((u64, u64, bool), Option<vectorcraft_doc::OrientedBox>)>,
    /// [`VectorcraftApp::selection_bounds`] for (document uid, revision).
    pub selection_bounds: Option<((u64, u64), Option<vectorcraft_geom::Rect>)>,
    /// The tools' cursors as OS cursor bitmaps.
    pub cursors: cursors::Images,
}

/// [`CanvasCache::slices`]: the layout of the slices of (document uid, revision).
pub type SliceCache = ((u64, u64), std::sync::Arc<Vec<vectorcraft_doc::SliceArea>>);

/// [`CanvasCache::print_tiling`]: the print tiling of (document uid, revision).
pub type PrintTilingCache = ((u64, u64), std::sync::Arc<Vec<vectorcraft_pdf::TilingPage>>);

#[derive(Clone, Debug, PartialEq)]
pub struct CacheKey {
    pub doc: usize,
    pub revision: u64,
    pub zoom: f64,
    pub cx: f64,
    pub cy: f64,
    pub w: u32,
    pub h: u32,
    pub outline: bool,
    pub trim: bool,
    pub ppp: f32,
    pub hidden: Vec<u64>,
    pub rot: f64,
    /// General › Anti-aliased Artwork.
    pub anti_alias: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Perf {
    pub frame_ms: f64,
    pub render_ms: f64,
    pub fps: f64,
}

pub struct VectorcraftApp {
    pub session: Session,
    pub ui: UiState,
    pub views: Vec<View>,
    pub services: Services,
    pub canvas: CanvasCache,
    pub perf: Perf,
    /// macOS: draw our own title strip under the traffic lights.
    pub integrated_titlebar: bool,
    /// Last applied effect (Effect → Apply Last Effect).
    pub last_effect: Option<(String, serde_json::Value)>,
    /// Commands run through [`Self::run`] so far: the native menu bar reads its rows again when it
    /// moves ([`native_menu::sync`]).
    run_count: u64,
    control_rx: Option<Receiver<ControlRequest>>,
    /// (token, path, reply, deadline ms): a window that isn't presented never delivers its frame.
    pending_screenshots: Vec<(u64, Option<String>, Sender<ControlResponse>, f64)>,
    queued_screenshots: Vec<(u64, f64, u32)>,
    screenshot_token: u64,
    /// Synthetic input events (from the control channel) injected one step per frame.
    pub synthetic: Vec<egui::Event>,
    styled: bool,
    fonts_ready: bool,
    /// The egui context the UI's textures were uploaded to (0: none yet; see
    /// [`Self::adopt_context`]).
    context: u64,
    /// Installed fonts added to the UI's for characters its own fonts lack (CJK names…).
    ui_fonts: ui_fonts::UiFonts,
    frame: u64,
    last_time: f64,
    /// Canvas rect of the last frame (screen points), for control-channel coordinate mapping.
    pub canvas_rect: Option<egui::Rect>,
    /// Hover position in document coordinates.
    pub hover_doc: Option<vectorcraft_geom::Point>,
    /// System clipboard: SVG to publish next frame, the last SVG we published (so pasting it back
    /// uses the lossless internal clipboard) and what came with a paste (a Paste event's text, a
    /// picture or file the host read: [`Self::paste_from_host`]).
    clipboard_out: Option<String>,
    clipboard_published: Option<String>,
    pub(crate) clipboard_in: Option<vectorcraft_engine::cmd::clipboard::Flavour>,
    /// A URL to open through egui next frame (when the host has no `open_url` service).
    pending_url: Option<String>,
    /// Windows and Linux: the window has no OS decorations, so the app bar is the title bar (drag,
    /// double-click to maximize, caption buttons) and invisible edge zones resize the window.
    pub custom_titlebar: bool,
    /// The graphics adapter the window renders with ("name (backend)"), as the host reports it:
    /// shown in Help › About and `ui.inspect` for GPU bug reports. `None` when unknown.
    pub graphics_adapter: Option<String>,
    /// File → Place: picked files, the place cursor's thumbnails, the Control bar's image details.
    pub place: place::PlaceState,
    /// The Paste commands can paste from the system clipboard alone: it holds something to paste
    /// (SVG without a `system_clipboard`) while the internal clipboard is empty. It enables the
    /// Paste menu items.
    pub(crate) system_paste: bool,
    /// When `system_paste` was last checked in line (app time, s; at most once per frame).
    system_paste_at: f64,
    /// The thread that checks the system clipboard for `system_paste` instead
    /// ([`Services::clipboard_probe`]).
    clipboard_probe: Option<clipboard_probe::Probe>,
    /// Keyboard pastes of something other than text (see [`shortcuts::PasteChord`]).
    pub(crate) paste_chord: shortcuts::PasteChord,
    /// Saves and exports running in the background (Preferences → File Handling).
    pub background: background::Background,
    /// Data Recovery's timer and startup question ([`recovery`]).
    pub recovery: recovery::Timer,
    /// Modifiers the keyboard holds (from the host's input), given back after synthetic input.
    host_modifiers: egui::Modifiers,
    /// Synthetic input set the modifiers egui holds (see [`Self::raw_input_hook`]).
    synthetic_modifiers: bool,
    /// The marked text the system IME last sent (`None` once it commits or clears). When the Type
    /// tool stops composing on its own (a click, a tool switch), the IME is told to drop it.
    pub(crate) ime_marked: Option<String>,
    /// The IME must drop its marked text (see [`Self::take_ime_discard`]).
    pub(crate) ime_discard: bool,
    /// A numeric field is being scrubbed: the document's edits meanwhile are one undo step
    /// ([`scrub::begin_frame`]).
    scrub_group: bool,
    /// Multiplayer CRDT bridge synchronizing session documents with remote peers.
    pub collab: Option<vectorcraft_engine::collab::CollabBridge>,
    /// Live remote cursor presence map, keyed by client id.
    pub remote_cursors: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<u64, PeerPresence>>>,
    /// This client's unique collaboration id.
    pub collab_client_id: u64,
    /// Last document revision that has been synchronized to/from the CRDT bridge.
    pub collab_last_synced_rev: u64,
    /// Last cursor point broadcast to remote peers.
    pub last_broadcast_cursor: Option<vectorcraft_geom::Point>,
    /// Last timestamp (app time seconds) at which cursor presence was broadcast.
    pub last_cursor_broadcast_at: f64,
    /// Custom peer nickname chosen by user in the UI.
    pub collab_custom_name: String,
    /// Timer (app time) until the "Link kopiert!" notification disappears.
    pub collab_share_copied_until: f64,
    /// Full shareable URL for this collaborative room.
    pub collab_share_url: String,
    /// Room identifier.
    pub collab_room_id: String,
}

/// Seconds between two looks at the system clipboard for [`VectorcraftApp::system_paste`].
const SYSTEM_CLIPBOARD_POLL: f64 = 0.25;

impl VectorcraftApp {
    pub fn new(mut session: Session, services: Services) -> Self {
        // The font menus and the first file opened need the installed fonts: catalog them now.
        vectorcraft_text::FontDb::global().scan_in_background();
        if let Some(store) = &services.recovery_store {
            session.recovery.set_store(store.clone());
        }
        let views = session.documents().iter().map(View::of).collect();
        let bridge = vectorcraft_engine::collab::CollabBridge::new();
        let collab_client_id: u64 = bridge.doc().client_id().get();
        let collab = if services.collab_inbox.is_some() || services.collab_outbox.is_some() { Some(bridge) } else { None };
        Self {
            session,
            ui: UiState::default(),
            views,
            services,
            canvas: CanvasCache {
                renderer: vectorcraft_render::Renderer::new(),
                texture: None,
                key: None,
                last_ms: 0.0,
                worker: None,
                worker_started: false,
                slices: None,
                print_tiling: None,
                selection_box: None,
                selection_bounds: None,
                cursors: Default::default(),
            },
            perf: Perf::default(),
            integrated_titlebar: false,
            last_effect: None,
            run_count: 0,
            clipboard_out: None,
            clipboard_published: None,
            clipboard_in: None,
            pending_url: None,
            control_rx: None,
            pending_screenshots: vec![],
            queued_screenshots: vec![],
            screenshot_token: 0,
            synthetic: vec![],
            styled: false,
            fonts_ready: false,
            context: 0,
            ui_fonts: Default::default(),
            frame: 0,
            last_time: 0.0,
            canvas_rect: None,
            hover_doc: None,
            custom_titlebar: false,
            graphics_adapter: None,
            place: Default::default(),
            system_paste: false,
            system_paste_at: f64::NEG_INFINITY,
            clipboard_probe: None,
            paste_chord: Default::default(),
            background: Default::default(),
            recovery: Default::default(),
            host_modifiers: Default::default(),
            synthetic_modifiers: false,
            ime_marked: None,
            ime_discard: false,
            scrub_group: false,
            collab,
            remote_cursors: Arc::new(Mutex::new(std::collections::HashMap::new())),
            collab_client_id,
            collab_last_synced_rev: 0,
            last_broadcast_cursor: None,
            last_cursor_broadcast_at: 0.0,
            collab_custom_name: String::new(),
            collab_share_copied_until: 0.0,
            collab_share_url: "http://192.168.10.19:8767/?room=c4ndy-studio".to_string(),
            collab_room_id: "c4ndy-studio".to_string(),
        }
    }

    pub fn with_control(mut self, rx: Receiver<ControlRequest>) -> Self {
        self.control_rx = Some(rx);
        self
    }

    /// Keep `views` aligned with the session's documents (a new one starts at its saved view).
    pub fn sync_views(&mut self) {
        let docs = self.session.documents();
        self.views.truncate(docs.len());
        self.views.extend(docs.iter().skip(self.views.len()).map(View::of));
    }

    pub fn view(&self) -> Option<&View> {
        self.session.active_index().and_then(|i| self.views.get(i))
    }
    pub fn view_mut(&mut self) -> Option<&mut View> {
        self.sync_views();
        let i = self.session.active_index()?;
        self.views.get_mut(i)
    }

    pub fn view_info(&self) -> ViewInfo {
        ViewInfo {
            zoom: self.view().map(|v| v.zoom).unwrap_or(1.0),
            outline: self.ui.view.outline,
            smart_guides: self.ui.view.smart_guides,
            guides: self.ui.view.guides,
            snap_to_grid: self.ui.view.snap_to_grid,
            snap_to_pixel: self.ui.view.snap_to_pixel,
            show_bbox: self.ui.view.bounding_box,
            snap_to_point: self.ui.view.snap_to_point,
            corner_widgets: self.ui.view.corner_widgets,
            screen: self.screen_frame(),
        }
    }

    /// The canvas on screen in document coordinates (none before it is laid out).
    pub fn screen_frame(&self) -> Option<vectorcraft_tools::ScreenFrame> {
        let (rect, view) = (self.canvas_rect?, self.view()?);
        let xf = canvas::Xf::new(rect, view);
        let px = |x: f32, y: f32| xf.delta_to_doc(egui::vec2(x, y));
        Some(vectorcraft_tools::ScreenFrame {
            origin: xf.to_doc(rect.left_top()),
            right: px(1.0, 0.0),
            down: px(0.0, 1.0),
            size: (f64::from(rect.width()), f64::from(rect.height())),
        })
    }

    /// Run a UI or engine command by id. The single entry point for every frontend path.
    pub fn run(&mut self, id: &str, params: Value) -> Result<Value, String> {
        self.run_count = self.run_count.wrapping_add(1);
        if let Some(r) = menus::run_ui_command(self, id, &params) {
            return r;
        }
        let mut params = params;
        if id.starts_with("edit.paste") {
            if let Err(e) = self.adopt_system_clipboard() {
                self.ui.status = e.clone();
                return Err(e);
            }
            // Paste (also without formatting) goes to the centre of the view.
            if matches!(id, "edit.paste" | "edit.pasteWithoutFormatting")
                && ["center", "dx", "dy"].iter().all(|k| params.get(k).is_none())
                && let Some(c) = self.view().filter(|v| v.fitted).map(|v| v.center)
                && let Some(p) = params.as_object_mut()
            {
                p.insert("center".into(), serde_json::json!([c.x, c.y]));
            }
            if let Some(r) = dialogs::swatch_conflict::ask(self, id, &params) {
                return r;
            }
        }
        // Native files carry the view they reopen at.
        if fileio::SaveMode::of(id).is_some() {
            io::remember_view(self);
        }
        let r = self.session.execute(id, &params).map_err(|e| e.to_string());
        if r.is_ok() && matches!(id, "edit.copy" | "edit.cut") {
            self.publish_clipboard();
        }
        self.sync_views();
        match &r {
            Err(e) => self.ui.status = e.clone(),
            Ok(v) => {
                if id == "file.new" {
                    self.ui.status.clear();
                    // New Document's Pixel preview mode (Overprint Preview is the engine's).
                    if v["previewMode"] == "pixel" {
                        self.ui.view.pixel_preview = true;
                    }
                }
                if id == "text.setStyle"
                    && let Some(font) = params.get("font").and_then(Value::as_str)
                {
                    let r = &mut self.ui.recent_fonts;
                    r.retain(|f| f != font);
                    r.insert(0, font.to_string());
                    r.truncate(MAX_RECENT_FONTS);
                }
            }
        }
        r
    }

    /// Open a link in the browser (Help → Discord, website, GitHub…).
    pub fn open_url(&mut self, url: &str) {
        match self.services.open_url.as_mut() {
            Some(open) => open(url),
            None => self.pending_url = Some(url.to_string()),
        }
        self.ui.status = format!("Opened {url}");
    }

    /// Run a Help link command and open the URL it returns.
    pub fn open_link(&mut self, id: &str) {
        if let Ok(v) = self.run(id, serde_json::json!({}))
            && let Some(u) = v["url"].as_str()
        {
            let u = u.to_string();
            self.open_url(&u);
        }
    }

    /// Select a tool (also used by the toolbar and shortcuts).
    pub fn select_tool(&mut self, id: &str) {
        let v = self.view_info();
        if let Err(e) = self.session.select_tool(id, v) {
            self.ui.status = e.to_string();
        }
        if let Some(g) = vectorcraft_tools::catalog::group_of(id)
            && let Some(slot) = self.ui.group_tool.get_mut(g)
        {
            *slot = id.to_string();
        }
        toolbar::remember(self, id);
        self.ui.flyout = None;
    }

    fn drain_control(&mut self, ctx: &egui::Context) {
        let Some(rx) = self.control_rx.take() else { return };
        while let Ok(req) = rx.try_recv() {
            let reply = req.reply.clone();
            // Guarded per request: a panic must not drop the channel (taken out of `self` above).
            let outcome = vectorcraft_engine::guard::catch_panic(|| control::handle(self, ctx, &req))
                .unwrap_or_else(|msg| control::err(format!("internal error: {msg} (please report this bug)")));
            match outcome {
                control::Outcome::Done(v) => {
                    let _ = reply.send(v);
                }
                control::Outcome::Screenshot { path } => {
                    self.screenshot_token += 1;
                    let token = self.screenshot_token;
                    let settle = ctx.global_style().animation_time as f64 * 2000.0 + 80.0;
                    self.queued_screenshots.push((token, now_ms() + settle, 0));
                    self.pending_screenshots.push((token, path, reply, now_ms() + settle + 8000.0));
                }
            }
        }
        self.control_rx = Some(rx);
    }

    fn issue_screenshots(&mut self, ctx: &egui::Context) {
        let now = now_ms();
        self.queued_screenshots.retain_mut(|(token, at, frames)| {
            *frames += 1;
            if now >= *at && *frames >= 3 {
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::new(*token)));
                false
            } else {
                true
            }
        });
        if !self.queued_screenshots.is_empty() || !self.pending_screenshots.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
    }

    fn collect_screenshots(&mut self, ctx: &egui::Context) {
        if self.pending_screenshots.is_empty() {
            return;
        }
        let events: Vec<_> = ctx.input(|i| {
            i.raw
                .events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::Screenshot { user_data, image, .. } => {
                        let token = user_data.data.as_ref().and_then(|d| d.downcast_ref::<u64>()).copied()?;
                        Some((token, image.clone()))
                    }
                    _ => None,
                })
                .collect()
        });
        for (token, image) in events {
            if let Some(i) = self.pending_screenshots.iter().position(|(t, ..)| *t == token) {
                let (_, path, reply, _) = self.pending_screenshots.remove(i);
                let _ = reply.send(control::save_screenshot(self, &image, path.as_deref()));
            }
        }
        let now = now_ms();
        self.pending_screenshots.retain(|(_, _, reply, deadline)| {
            if now < *deadline {
                return true;
            }
            let _ = reply.send(serde_json::json!({
                "ok": false,
                "error": "no frame was presented (screen locked, window minimized or fully covered); ui.render still renders the artboard"
            }));
            false
        });
    }

    /// Did the Type tool end an IME composition on its own this frame? Interrupting the IME
    /// through egui (`should_interrupt_composition`) doesn't reach the macOS input context, which
    /// keeps the marked text and types it again into the next composition: the host discards it.
    pub fn take_ime_discard(&mut self) -> bool {
        std::mem::take(&mut self.ime_discard)
    }

    /// Show a transient status message.
    pub fn status(&mut self, s: impl Into<String>) {
        self.ui.status = s.into();
    }

    fn drain_inbox(&mut self) {
        let arrived: Vec<(String, Vec<u8>)> =
            self.services.inbox.as_ref().map(|q| std::mem::take(&mut *q.lock().unwrap_or_else(|e| e.into_inner()))).unwrap_or_default();
        for (name, bytes) in arrived {
            if let Err(e) = io::open_bytes(self, &name, &bytes, None) {
                self.status(format!("Couldn't open {name}: {e}"));
            }
        }
        place::drain(self);
    }

    pub fn collab_peer_name(&self) -> String {
        let trimmed = self.collab_custom_name.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
        format!("User {}", self.collab_client_id % 1000)
    }

    pub fn copy_share_link(&mut self, ctx: &egui::Context, now: f64) {
        self.collab_share_copied_until = now + 2.5;
        ctx.copy_text(self.collab_share_url.clone());
        ctx.request_repaint();
    }

    pub fn collab_peer_color(&self) -> [u8; 4] {
        const PEER_COLORS: [[u8; 4]; 8] = [
            [242, 78, 30, 255],  // Figma orange/red
            [255, 114, 98, 255], // Coral
            [162, 89, 255, 255], // Purple
            [24, 160, 251, 255], // Blue
            [10, 207, 131, 255], // Green
            [255, 192, 67, 255], // Yellow/Amber
            [235, 47, 150, 255], // Magenta
            [19, 194, 194, 255], // Cyan
        ];
        PEER_COLORS[(self.collab_client_id as usize) % PEER_COLORS.len()]
    }

    pub fn handle_collab(&mut self, ctx: &egui::Context, now: f64) {
        if self.collab.is_none() && (self.services.collab_inbox.is_some() || self.services.collab_outbox.is_some()) {
            self.collab = Some(vectorcraft_engine::collab::CollabBridge::with_client_id(self.collab_client_id));
        }

        // Drain incoming messages
        let incoming = self
            .services
            .collab_inbox
            .as_ref()
            .map(|q| {
                let mut g = q.lock().unwrap_or_else(|e| e.into_inner());
                std::mem::take(&mut *g)
            })
            .unwrap_or_default();

        for msg in incoming {
            match msg {
                CollabIncoming::Update(bytes) => {
                    if let Some(collab) = &mut self.collab {
                        if let Err(e) = collab.apply_remote_update(&mut self.session, &bytes) {
                            log::warn!("collab: apply remote update error: {e}");
                        } else {
                            if let Some(st) = self.session.active() {
                                self.collab_last_synced_rev = st.revision;
                            }
                            if self.views.len() != self.session.documents().len() {
                                self.views = self.session.documents().iter().map(View::of).collect();
                            }
                            ctx.request_repaint();
                        }
                    }
                }
                CollabIncoming::Presence(p) => {
                    if p.client_id != self.collab_client_id {
                        let mut cursors = self.remote_cursors.lock().unwrap_or_else(|e| e.into_inner());
                        cursors.insert(p.client_id, p);
                        ctx.request_repaint();
                    }
                }
            }
        }

        // Prune stale cursors (older than 10 seconds)
        {
            let mut cursors = self.remote_cursors.lock().unwrap_or_else(|e| e.into_inner());
            cursors.retain(|_, p| now - p.last_seen < 10.0);
        }

        // Check outgoing local document changes
        if let Some(st) = self.session.active() {
            if st.revision > self.collab_last_synced_rev {
                if let Some(collab) = &mut self.collab {
                    match collab.sync_from_session(&self.session) {
                        Ok(bytes) => {
                            self.collab_last_synced_rev = st.revision;
                            if !bytes.is_empty() {
                                if let Some(outbox) = &self.services.collab_outbox {
                                    let mut g = outbox.lock().unwrap_or_else(|e| e.into_inner());
                                    g.push(CollabOutgoing::Update(bytes));
                                }
                            }
                        }
                        Err(e) => {
                            log::warn!("collab: sync from session error: {e}");
                        }
                    }
                }
            }
        }

        // Check outgoing cursor updates
        let cursor_changed = self.hover_doc != self.last_broadcast_cursor;
        let dt = now - self.last_cursor_broadcast_at;
        if (cursor_changed && dt >= 0.033) || dt >= 1.0 {
            if cursor_changed || self.hover_doc.is_some() {
                if let Some(outbox) = &self.services.collab_outbox {
                    let presence = PeerPresence {
                        client_id: self.collab_client_id,
                        name: self.collab_peer_name(),
                        color: self.collab_peer_color(),
                        cursor: self.hover_doc,
                        last_seen: now,
                    };
                    let mut g = outbox.lock().unwrap_or_else(|e| e.into_inner());
                    g.push(CollabOutgoing::Presence(presence));
                    self.last_broadcast_cursor = self.hover_doc;
                    self.last_cursor_broadcast_at = now;
                }
            }
        } else if cursor_changed {
            ctx.request_repaint_after(std::time::Duration::from_millis(33));
        }
    }
}

pub fn now_ms() -> f64 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64() * 1000.0).unwrap_or(0.0)
    }
    #[cfg(target_arch = "wasm32")]
    {
        0.0
    }
}

/// The most fonts Type › Recent Fonts lists (Preferences › Type › Number of Recent Fonts).
pub const MAX_RECENT_FONTS: usize = 15;

impl VectorcraftApp {
    /// Type › Recent Fonts: the fonts used last, newest first, as many as Preferences › Type ›
    /// Number of Recent Fonts says.
    pub fn recent_fonts(&self) -> &[String] {
        let n = usize::try_from(self.session.prefs.recent_fonts_count).unwrap_or(MAX_RECENT_FONTS).clamp(1, MAX_RECENT_FONTS);
        self.ui.recent_fonts.get(..n).unwrap_or(&self.ui.recent_fonts)
    }

    /// The selection's bounding box, rotated with rotated objects ([`Session::transform_box`]). The
    /// canvas and the transform fields read it every frame: it is measured once per revision.
    pub fn selection_box(&mut self) -> Option<vectorcraft_doc::OrientedBox> {
        let st = self.session.active()?;
        let key = (st.uid, st.revision, self.session.prefs.use_preview_bounds);
        if let Some((k, b)) = self.canvas.selection_box
            && k == key
        {
            return b;
        }
        let b = self.session.transform_box(&st.selection.objects);
        self.canvas.selection_box = Some((key, b));
        b
    }

    /// The selection's visual bounds (stroke and effects included), square to the page: measured
    /// once per revision, as the canvas reads it every frame (a traced photo selects a group of
    /// hundreds of thousands of paths).
    pub fn selection_bounds(&mut self) -> Option<vectorcraft_geom::Rect> {
        let st = self.session.active()?;
        let key = (st.uid, st.revision);
        if let Some((k, b)) = self.canvas.selection_bounds
            && k == key
        {
            return b;
        }
        let b = st.doc.bounds_of(&st.selection.objects, true);
        self.canvas.selection_bounds = Some((key, b));
        b
    }
}

/// eframe isn't a dependency of this crate (the host owns the event loop); these entry points are
/// called from the host's `eframe::App` impl.
impl VectorcraftApp {
    /// The language the UI is drawn in: the Preferences dialog's choice while it is open (so a
    /// change shows before OK), else the `interfaceLanguage` preference (`auto` = the system's).
    pub fn ui_language(&self) -> i18n::Lang {
        let editing =
            self.ui.dialog.as_ref().filter(|d| d.kind == "preferences").and_then(|d| d.fields.get("interfaceLanguage")).and_then(Value::as_str);
        i18n::Lang::from_pref(editing.unwrap_or(&self.session.prefs.interface_language))
    }

    /// Per-frame logic before layout (control channel, shortcuts, inbox). A bug that panics costs
    /// one frame and shows an error, instead of closing the app with unsaved work.
    pub fn logic(&mut self, ctx: &egui::Context) {
        if let Err(msg) = vectorcraft_engine::guard::catch_panic(|| self.logic_frame(ctx)) {
            self.status(format!("Internal error: {msg} (please report this bug)"));
        }
    }

    fn logic_frame(&mut self, ctx: &egui::Context) {
        let lang = self.ui_language();
        i18n::set_current(lang);
        // The engine gives new type the Japanese defaults while the UI is in Japanese.
        if self.session.ui_language.as_deref() != Some(lang.code()) {
            self.session.ui_language = Some(lang.code().to_string());
        }
        self.adopt_context(ctx);
        if !self.styled {
            theme::install_fonts(ctx);
            theme::apply(ctx, self.ui.brightness);
            egui_extras::install_image_loaders(ctx);
            self.styled = true;
        } else {
            self.fonts_ready = true;
        }
        self.frame += 1;
        let now = ctx.input(|i| i.time);
        let dt = now - self.last_time;
        if dt > 0.0 {
            self.perf.fps = self.perf.fps * 0.9 + (1.0 / dt).min(240.0) * 0.1;
        }
        self.last_time = now;
        self.sync_views();
        // Read the system clipboard only when that alone decides whether Paste is enabled. A
        // background thread reads it where the host installs one (an unresponsive owner must never
        // stall the frame loop); otherwise, or once that thread is gone, it is read here, at most a
        // few times a second (opening it locks it against other apps on some systems).
        let wanted = self.session.clipboard.is_empty() && self.session.active().is_some();
        if let Some(make) = self.services.clipboard_probe.take() {
            self.clipboard_probe = clipboard_probe::Probe::start(make, ctx.clone());
        }
        if let Some(pasteable) = self.clipboard_probe.as_mut().and_then(|p| p.pasteable(wanted)) {
            self.system_paste = pasteable;
        } else if !(0.0..SYSTEM_CLIPBOARD_POLL).contains(&(now - self.system_paste_at)) {
            self.clipboard_probe = None;
            self.system_paste_at = now;
            self.system_paste = wanted && self.system_clipboard_pasteable();
        }
        background::poll(self);
        if !self.background.jobs.is_empty() {
            // Keep the status bar's progress moving and pick the result up when it arrives.
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        // The window's close button (or the system quitting the app) asks about unsaved documents,
        // once the saves running in the background are done.
        if ctx.input(|i| i.viewport().close_requested()) {
            background::wait_all(self);
            if unsaved::any_dirty(self) {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                if let Err(e) = unsaved::close_all(self, "quit") {
                    self.status(e);
                }
            } else {
                // Quitting with nothing unsaved: no copies to leave behind.
                vectorcraft_engine::cmd::recovery::forget_all(&mut self.session);
            }
        }
        if let Some(wait) = recovery::frame(self, now) {
            // The timer and heartbeat run in an idle window too.
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(wait));
        }
        shortcut_editor::sync(&self.ui);
        prefs_dialog::apply_runtime(self, ctx);
        self.drain_control(ctx);
        if !self.synthetic.is_empty() {
            ctx.request_repaint();
        }
        self.collect_screenshots(ctx);
        self.issue_screenshots(ctx);
        if let Some(u) = self.pending_url.take() {
            ctx.open_url(egui::OpenUrl::new_tab(u));
        }
        if let Some(t) = self.clipboard_out.take() {
            ctx.copy_text(t);
        }
        self.drain_inbox();
        self.handle_collab(ctx, now);
        native_menu::run(self, ctx);
        if self.fonts_ready {
            shortcuts::handle(self, ctx);
        }
        // Native only: the web host reads dropped files asynchronously and feeds the inboxes.
        #[cfg(not(target_arch = "wasm32"))]
        self.take_dropped_files(ctx);
    }

    /// Files dropped on the window: documents opened, pictures and text placed on the canvas
    /// ([`Self::drop_target`]).
    #[cfg(not(target_arch = "wasm32"))]
    fn take_dropped_files(&mut self, ctx: &egui::Context) {
        let (dropped, shift) = ctx.input(|i| (i.raw.dropped_files.clone(), i.modifiers.shift));
        if dropped.is_empty() {
            return;
        }
        let pos = place::drag_pos(ctx);
        let mut files = vec![];
        for f in dropped {
            let path = Some(f.path().to_string_lossy().to_string()).filter(|s| !s.is_empty());
            let name = path.as_deref().map_or_else(|| "dropped".into(), vectorcraft_engine::cmd::fileio::file_name);
            let target = self.drop_target(&name, pos, shift);
            // A file placed by its path is read by the engine.
            let bytes = if path.is_some() && target != place::DropTarget::Open { Ok(vec![]) } else { f.bytes() };
            match bytes {
                Ok(b) => files.push((target, (name, path, b))),
                Err(e) => self.status(format!("Couldn't read {name}: {e}")),
            }
        }
        place::drop_files(self, files);
    }

    /// Fonts installed or removed while the app was in the background are listed when it comes
    /// back (Refresh Font List by itself): a look at the font folders, a scan only when they changed.
    fn refresh_installed_fonts(&mut self) {
        if vectorcraft_text::FontDb::global().installed_fonts_changed() {
            // A failure shows in the status bar, as the menu item's does.
            let _ = self.run("text.rescanFonts", json!({}));
        }
    }

    /// Inject synthetic events (one press/release step or wheel turn per frame). Handlers read the
    /// modifiers egui holds (`i.modifiers`), so a synthetic key, button or wheel turn holds its own
    /// for the frames it spans (a drag's moves included); the keyboard's come back after.
    pub fn raw_input_hook(&mut self, raw: &mut egui::RawInput) {
        // The native menu's key equivalents become the input their keys make, ahead of what came
        // after them (see `native_menu`).
        let keys = self.services.native_menu.as_mut().map(native_menu::NativeMenu::take_keys).unwrap_or_default();
        if !keys.is_empty() {
            let events: Vec<egui::Event> = keys.into_iter().flat_map(|k| native_menu::key_events(k, || self.system_clipboard_text())).collect();
            raw.events.splice(0..0, events);
        }
        for e in &raw.events {
            match e {
                egui::Event::ModifiersChanged(m) => self.host_modifiers = *m,
                egui::Event::WindowFocused(false) => self.host_modifiers = egui::Modifiers::NONE,
                egui::Event::WindowFocused(true) => self.refresh_installed_fonts(),
                _ => {}
            }
        }
        let Some(first) = self.synthetic.first() else {
            if std::mem::take(&mut self.synthetic_modifiers) {
                raw.events.push(egui::Event::ModifiersChanged(self.host_modifiers));
            }
            return;
        };
        // Pointer events go one per frame so egui sees presses, drags and releases as real input;
        // keyboard sequences go up to the key release.
        let n = match first {
            egui::Event::PointerMoved(_) | egui::Event::PointerButton { .. } | egui::Event::MouseWheel { .. } => 1,
            _ => self.synthetic.iter().position(|e| matches!(e, egui::Event::Key { pressed: false, .. })).map_or(self.synthetic.len(), |i| i + 1),
        };
        if let egui::Event::PointerMoved(p) | egui::Event::PointerButton { pos: p, .. } = first {
            raw.events.push(egui::Event::PointerMoved(*p));
        }
        let (now, later) = self.synthetic.split_at(n.min(self.synthetic.len()));
        // This frame's key or button, else the button a drag holds down (released later).
        let held = now
            .iter()
            .find_map(|e| match e {
                egui::Event::Key { modifiers, .. } | egui::Event::PointerButton { modifiers, .. } | egui::Event::MouseWheel { modifiers, .. } => {
                    Some(*modifiers)
                }
                _ => None,
            })
            .or_else(|| match later.iter().find(|e| matches!(e, egui::Event::PointerButton { .. })) {
                Some(egui::Event::PointerButton { pressed: false, modifiers, .. }) => Some(*modifiers),
                _ => None,
            });
        match held {
            Some(m) => {
                raw.events.push(egui::Event::ModifiersChanged(m));
                self.synthetic_modifiers = true;
            }
            None if std::mem::take(&mut self.synthetic_modifiers) => raw.events.push(egui::Event::ModifiersChanged(self.host_modifiers)),
            None => {}
        }
        raw.events.extend(self.synthetic.drain(..n));
    }

    /// Lay out the whole window.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        if let Err(msg) = vectorcraft_engine::guard::catch_panic(|| self.ui_frame(ui)) {
            self.status(format!("Internal error: {msg} (please report this bug)"));
        }
    }

    fn ui_frame(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        if !self.fonts_ready {
            ctx.request_repaint();
            return;
        }
        let t0 = now_ms();
        scrub::begin_frame(self, &ctx);
        font_menu::end_stale_preview(self, &ctx);
        floating::track(self, &ctx);
        let t = theme::Tokens::get(&ctx);
        if self.ui.screen_mode < 2 {
            chrome::app_bar(self, ui);
            if self.ui.control_bar {
                chrome::control_bar(self, ui);
            }
        }
        if self.ui.status_bar && self.ui.screen_mode < 3 {
            chrome::status_bar(self, ui);
            chrome::hint_bar(self, ui);
        }
        if self.ui.toolbar && self.ui.screen_mode < 3 {
            toolbar::show(self, ui);
        }
        if self.ui.dock && self.ui.screen_mode < 3 {
            dock::show(self, ui);
        }
        egui::CentralPanel::default().frame(egui::Frame::NONE.fill(t.pasteboard)).show(ui, |ui| {
            if self.ui.screen_mode < 3 {
                chrome::doc_tabs(self, ui);
            }
            canvas::show(self, ui);
        });
        dock::floating_panel(self, &ctx);
        floating::show(self, &ctx);
        panels::library_panel::show_window(self, &ctx);
        dialogs::show(self, &ctx);
        palette::show(self, &ctx);
        if self.custom_titlebar {
            titlebar::resize_zones(ui);
        }
        self.ui_fonts.frame(&ctx);
        scrub::end_frame(self, &ctx);
        native_menu::sync(self, &ctx);
        self.perf.frame_ms = now_ms() - t0;
        let _ = json!(null);
    }
}

#[cfg(test)]
mod tests_collab {
    use crate::{CollabIncoming, CollabOutgoing, PeerPresence, Services, VectorcraftApp};
    use serde_json::json;
    use std::sync::{Arc, Mutex};
    use vectorcraft_engine::Session;
    use vectorcraft_geom::Point;

    #[test]
    fn test_peer_presence_serde_roundtrip() {
        let presence = PeerPresence {
            client_id: 12345,
            name: "Designer Alice".to_string(),
            color: [24, 160, 251, 255],
            cursor: Some(Point::new(450.5, 320.25)),
            last_seen: 100.5,
        };

        let serialized = serde_json::to_string(&presence).expect("serialize presence");
        let deserialized: PeerPresence = serde_json::from_str(&serialized).expect("deserialize presence");

        assert_eq!(deserialized.client_id, presence.client_id);
        assert_eq!(deserialized.name, presence.name);
        assert_eq!(deserialized.color, presence.color);
        assert_eq!(deserialized.cursor, presence.cursor);
        assert_eq!(deserialized.last_seen, presence.last_seen);
    }

    #[test]
    fn test_collab_two_peer_sync_and_echo_loop_prevention() {
        let inbox_a = Arc::new(Mutex::new(Vec::new()));
        let outbox_a = Arc::new(Mutex::new(Vec::new()));
        let inbox_b = Arc::new(Mutex::new(Vec::new()));
        let outbox_b = Arc::new(Mutex::new(Vec::new()));

        let services_a = Services { collab_inbox: Some(inbox_a.clone()), collab_outbox: Some(outbox_a.clone()), ..Default::default() };
        let services_b = Services { collab_inbox: Some(inbox_b.clone()), collab_outbox: Some(outbox_b.clone()), ..Default::default() };

        let mut app_a = VectorcraftApp::new(Session::new(), services_a);
        let mut app_b = VectorcraftApp::new(Session::new(), services_b);

        app_a.run("file.new", json!({"width": 800, "height": 600})).expect("file.new a");
        app_b.run("file.new", json!({"width": 800, "height": 600})).expect("file.new b");

        let ctx = egui::Context::default();

        // 1. Peer A draws a shape
        app_a.run("shape.rectangle", json!({"x": 100.0, "y": 150.0, "width": 200.0, "height": 100.0})).expect("shape.rectangle");

        // Sync peer A
        app_a.handle_collab(&ctx, 1.0);

        // Check peer A pushed an update to outbox
        let mut out_a = outbox_a.lock().unwrap();
        assert_eq!(out_a.len(), 1, "peer A must produce one outgoing update");
        let update_bytes = match out_a.pop().unwrap() {
            CollabOutgoing::Update(b) => b,
            _ => panic!("expected CollabOutgoing::Update"),
        };
        drop(out_a);

        // 2. Deliver update to Peer B
        inbox_b.lock().unwrap().push(CollabIncoming::Update(update_bytes));
        app_b.handle_collab(&ctx, 1.0);

        // Check peer B ingested shape
        let st_b = app_b.session.active().expect("active doc on b");
        let mut path_count = 0;
        st_b.doc.walk(|n| {
            if matches!(n.kind, vectorcraft_doc::NodeKind::Path { .. }) {
                path_count += 1;
            }
        });
        assert_eq!(path_count, 1, "peer B must have received the shape");

        // Check peer B DID NOT produce an echo update
        let out_b = outbox_b.lock().unwrap();
        assert!(out_b.is_empty(), "peer B must not echo remote update back to outbox");
    }

    #[test]
    fn test_collab_presence_broadcast_and_reception() {
        let inbox = Arc::new(Mutex::new(Vec::new()));
        let outbox = Arc::new(Mutex::new(Vec::new()));

        let services = Services { collab_inbox: Some(inbox.clone()), collab_outbox: Some(outbox.clone()), ..Default::default() };

        let mut app = VectorcraftApp::new(Session::new(), services);
        let ctx = egui::Context::default();

        // 1. Remote peer presence arrives in inbox
        let peer_presence = PeerPresence {
            client_id: 999,
            name: "Remote Bob".to_string(),
            color: [10, 207, 131, 255],
            cursor: Some(Point::new(200.0, 300.0)),
            last_seen: 5.0,
        };
        inbox.lock().unwrap().push(CollabIncoming::Presence(peer_presence.clone()));

        app.handle_collab(&ctx, 5.0);

        // Check remote peer cursor is stored
        {
            let cursors = app.remote_cursors.lock().unwrap();
            assert!(cursors.contains_key(&999));
            assert_eq!(cursors.get(&999).unwrap().name, "Remote Bob");
        }

        // 2. Local cursor movement triggers outgoing presence
        app.hover_doc = Some(Point::new(150.0, 250.0));
        app.handle_collab(&ctx, 6.0);

        let out = outbox.lock().unwrap();
        assert!(!out.is_empty(), "local cursor move should push presence to outbox");
        let presence_out = out
            .iter()
            .find_map(|msg| match msg {
                CollabOutgoing::Presence(p) => Some(p.clone()),
                _ => None,
            })
            .expect("presence in outbox");
        assert_eq!(presence_out.client_id, app.collab_client_id);
        assert_eq!(presence_out.cursor, Some(Point::new(150.0, 250.0)));
    }

    #[test]
    fn test_remote_cursor_canvas_paint() {
        let app = VectorcraftApp::new(Session::new(), Services::default());
        let peer_presence = PeerPresence {
            client_id: 888,
            name: "Carol".to_string(),
            color: [255, 114, 98, 255],
            cursor: Some(Point::new(300.0, 400.0)),
            last_seen: 10.0,
        };
        app.remote_cursors.lock().unwrap().insert(888, peer_presence);

        let ctx = egui::Context::default();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
            let painter = ui.painter_at(rect);
            let view = crate::state::View::default();
            let xf = crate::canvas::Xf::new(rect, &view);

            crate::canvas::paint_remote_cursors(&app, &painter, &xf);
        });
        out.textures_delta.clear();
    }

    #[test]
    fn test_cursor_coordinate_invariance_pan_zoom_rotation() {
        let doc_pt = Point::new(250.0, 350.0);
        let rect = egui::Rect::from_min_size(egui::pos2(100.0, 50.0), egui::vec2(1200.0, 800.0));

        let zooms = [0.0313, 0.1, 0.5, 1.0, 2.0, 5.0, 10.0, 100.0, 640.0];
        let centers = [
            Point::new(0.0, 0.0),
            Point::new(250.0, 350.0),
            Point::new(-1000.0, 5000.0),
            Point::new(10000.0, -10000.0),
        ];
        let rotations = [0.0, 15.0, 45.0, 90.0, 180.0, 270.0];

        for &zoom in &zooms {
            for &center in &centers {
                for &rotation in &rotations {
                    let view = crate::state::View {
                        zoom,
                        center,
                        rotation,
                        ..Default::default()
                    };
                    let xf = crate::canvas::Xf::new(rect, &view);

                    let screen_pos = xf.to_screen(doc_pt);
                    let recovered_doc = xf.to_doc(screen_pos);

                    let err_x = (recovered_doc.x - doc_pt.x).abs();
                    let err_y = (recovered_doc.y - doc_pt.y).abs();
                    assert!(
                        err_x < 1e-2 && err_y < 1e-2,
                        "Roundtrip error at zoom={zoom}, center={center:?}, rot={rotation}: got ({err_x}, {err_y})"
                    );

                    let pts = crate::cursors::arrow_points(screen_pos);
                    assert_eq!(pts[0], screen_pos, "Arrow tip must be exactly at projected screen_pos");

                    let pan_delta = vectorcraft_geom::Vec2::new(50.0, -30.0);
                    let panned_view = crate::state::View {
                        zoom,
                        center: center + pan_delta,
                        rotation,
                        ..Default::default()
                    };
                    let panned_xf = crate::canvas::Xf::new(rect, &panned_view);
                    let panned_screen_pos = panned_xf.to_screen(doc_pt);

                    let panned_recovered = panned_xf.to_doc(panned_screen_pos);
                    assert!(
                        (panned_recovered.x - doc_pt.x).abs() < 1e-2 && (panned_recovered.y - doc_pt.y).abs() < 1e-2,
                        "Panned cursor must remain locked to doc_pt"
                    );
                }
            }
        }
    }

    #[test]
    fn test_remote_cursor_clipping_outside_canvas_rect() {
        let app = VectorcraftApp::new(Session::new(), Services::default());
        let inside = PeerPresence {
            client_id: 101,
            name: "Inside".to_string(),
            color: [255, 0, 0, 255],
            cursor: Some(Point::new(400.0, 300.0)),
            last_seen: 1.0,
        };
        let outside_tl = PeerPresence {
            client_id: 102,
            name: "OutsideTL".to_string(),
            color: [0, 255, 0, 255],
            cursor: Some(Point::new(-5000.0, -5000.0)),
            last_seen: 1.0,
        };
        let outside_br = PeerPresence {
            client_id: 103,
            name: "OutsideBR".to_string(),
            color: [0, 0, 255, 255],
            cursor: Some(Point::new(50000.0, 50000.0)),
            last_seen: 1.0,
        };
        {
            let mut g = app.remote_cursors.lock().unwrap();
            g.insert(101, inside);
            g.insert(102, outside_tl);
            g.insert(103, outside_br);
        }

        let ctx = egui::Context::default();
        let rect = egui::Rect::from_min_size(egui::pos2(100.0, 100.0), egui::vec2(800.0, 600.0));
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            let painter = ui.painter_at(rect);
            assert_eq!(painter.clip_rect(), rect, "painter_at must set clip_rect to canvas rect");

            let view = crate::state::View {
                zoom: 1.0,
                center: Point::new(400.0, 300.0),
                ..Default::default()
            };
            let xf = crate::canvas::Xf::new(rect, &view);
            crate::canvas::paint_remote_cursors(&app, &painter, &xf);
        });

        for clipped_shape in &out.shapes {
            assert!(
                rect.contains_rect(clipped_shape.clip_rect) || clipped_shape.clip_rect == rect,
                "Every shape emitted must be clipped by the canvas rect: shape clip_rect is {:?}",
                clipped_shape.clip_rect
            );
        }
        out.textures_delta.clear();
    }

    #[test]
    fn test_remote_cursor_robustness_offscreen_jitter_and_extreme_coords() {
        let app = VectorcraftApp::new(Session::new(), Services::default());
        let ctx = egui::Context::default();
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1000.0, 1000.0));
        let view = crate::state::View::default();
        let xf = crate::canvas::Xf::new(rect, &view);

        let extremes = [
            Point::new(1e9, 1e9),
            Point::new(-1e9, -1e9),
            Point::new(1e15, -1e15),
            Point::new(0.0, 0.0),
            Point::new(-0.0, -0.0),
            Point::new(f64::MIN_POSITIVE, f64::MIN_POSITIVE),
        ];

        for (idx, &pt) in extremes.iter().enumerate() {
            let peer = PeerPresence {
                client_id: (idx + 1000) as u64,
                name: format!("Peer-{}", idx),
                color: [100, 150, 200, 255],
                cursor: Some(pt),
                last_seen: 1.0,
            };
            app.remote_cursors.lock().unwrap().insert(peer.client_id, peer);
        }

        let weird_names = [
            "",
            "A",
            "A very long peer name that might exceed normal badge sizes and wraps around or gets truncated by ui layout engine",
            "🎉 🚀 👾 🦀",
            "مرحبا بالعالم",
            "你好世界",
            "Line1\nLine2\nLine3",
            "\t\r\n",
        ];
        for (idx, &name) in weird_names.iter().enumerate() {
            let peer = PeerPresence {
                client_id: (idx + 2000) as u64,
                name: name.to_string(),
                color: [200, 50, 100, 255],
                cursor: Some(Point::new(100.0 + idx as f64 * 10.0, 100.0 + idx as f64 * 10.0)),
                last_seen: 1.0,
            };
            app.remote_cursors.lock().unwrap().insert(peer.client_id, peer);
        }

        // Test render run
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            let painter = ui.painter_at(rect);
            crate::canvas::paint_remote_cursors(&app, &painter, &xf);
        });
        out.textures_delta.clear();

        // Jitter stress test: 500 fast updates
        for i in 0..500 {
            let x = ((i * 17) % 1000) as f64;
            let y = ((i * 31) % 1000) as f64;
            let peer = PeerPresence {
                client_id: 3000,
                name: "Jittery".to_string(),
                color: [50, 200, 50, 255],
                cursor: Some(Point::new(x, y)),
                last_seen: i as f64,
            };
            app.remote_cursors.lock().unwrap().insert(3000, peer);

            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                let painter = ui.painter_at(rect);
                crate::canvas::paint_remote_cursors(&app, &painter, &xf);
            });
            out.textures_delta.clear();
        }
    }

    #[test]
    fn test_remote_cursor_nan_and_inf_handling() {
        let app = VectorcraftApp::new(Session::new(), Services::default());
        let ctx = egui::Context::default();
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
        let view = crate::state::View::default();
        let xf = crate::canvas::Xf::new(rect, &view);

        let nan_peer = PeerPresence {
            client_id: 4001,
            name: "NaN Peer".to_string(),
            color: [255, 0, 0, 255],
            cursor: Some(Point::new(f64::NAN, 100.0)),
            last_seen: 1.0,
        };
        let inf_peer = PeerPresence {
            client_id: 4002,
            name: "Inf Peer".to_string(),
            color: [0, 255, 0, 255],
            cursor: Some(Point::new(f64::INFINITY, f64::NEG_INFINITY)),
            last_seen: 1.0,
        };
        app.remote_cursors.lock().unwrap().insert(4001, nan_peer);
        app.remote_cursors.lock().unwrap().insert(4002, inf_peer);

        // Run UI and ensure no panic occurs
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            let painter = ui.painter_at(rect);
            crate::canvas::paint_remote_cursors(&app, &painter, &xf);
        });
        out.textures_delta.clear();
    }
}
