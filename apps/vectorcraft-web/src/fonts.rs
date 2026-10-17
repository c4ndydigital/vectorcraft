//! Craft-fonts faces served beside the wasm (`fonts/<sha16>/…`, copied by
//! `cargo xtask web-fonts`) instead of embedded: [`load_startup_fonts`] fetches the `startup`
//! ones before the app starts, [`spawn_background_fonts`] the rest once it runs. The browser checks
//! each file against its SHA-256 (Subresource Integrity). A face that fails is logged and skipped:
//! the app runs without it. With `WEB_FONTS` empty (built without craft-fonts) both do nothing.

use std::cell::Cell;
use std::rc::Rc;

use vectorcraft_text::{FontDb, WEB_FONTS, WebFont};
use wasm_bindgen::JsCast as _;
use wasm_bindgen::closure::Closure;
use wasm_bindgen_futures::JsFuture;

/// Per startup face: the page shows its loading text meanwhile.
const STARTUP_TIMEOUT_MS: i32 = 20_000;
/// Per background face.
const BACKGROUND_TIMEOUT_MS: i32 = 120_000;
/// Background fetches at once, leaving the connection to the app's own requests.
const BACKGROUND_PARALLEL: usize = 4;

thread_local! {
    /// The background fetch runs once per page, though a graphics restart creates the app again.
    static BACKGROUND_STARTED: Cell<bool> = const { Cell::new(false) };
}

/// Fetch and register the `startup` faces (one after the other). Returns how many loaded.
pub async fn load_startup_fonts() -> usize {
    let mut loaded = 0;
    for f in WEB_FONTS.iter().filter(|f| f.startup) {
        if load(f, STARTUP_TIMEOUT_MS).await {
            loaded += 1;
        }
    }
    loaded
}

/// Fetch and register the `background` faces, [`BACKGROUND_PARALLEL`] at a time, asking `ctx` for
/// a frame after each so the font menus and the canvas pick it up. Only the first call does
/// anything.
pub fn spawn_background_fonts(ctx: egui::Context) {
    if BACKGROUND_STARTED.with(|s| s.replace(true)) {
        return;
    }
    let queue: Rc<Vec<&'static WebFont>> = Rc::new(WEB_FONTS.iter().filter(|f| !f.startup).collect());
    let next = Rc::new(Cell::new(0usize));
    for _ in 0..BACKGROUND_PARALLEL.min(queue.len()) {
        let (queue, next, ctx) = (queue.clone(), next.clone(), ctx.clone());
        wasm_bindgen_futures::spawn_local(async move {
            loop {
                let i = next.get();
                next.set(i + 1);
                let Some(f) = queue.get(i) else { break };
                if load(f, BACKGROUND_TIMEOUT_MS).await {
                    ctx.request_repaint();
                }
            }
        });
    }
}

/// Fetch one face and add it to the font database. False (and a console warning) on failure.
async fn load(f: &WebFont, timeout_ms: i32) -> bool {
    let bytes = match fetch(f.url, f.integrity, timeout_ms).await {
        Ok(bytes) => bytes,
        Err(e) => {
            log::warn!("web font {} {}: {e}", f.family, f.style);
            return false;
        }
    };
    if FontDb::global().add_font(bytes) == 0 {
        log::warn!("web font {} {}: not a font, or already loaded", f.family, f.style);
        return false;
    }
    true
}

/// GET `url` (relative to the page) with Subresource Integrity `integrity`, aborted after
/// `timeout_ms`.
async fn fetch(url: &str, integrity: &str, timeout_ms: i32) -> Result<Vec<u8>, String> {
    let window = web_sys::window().ok_or("no window")?;
    let abort = web_sys::AbortController::new().map_err(js)?;
    let init = web_sys::RequestInit::new();
    init.set_method("GET");
    init.set_integrity(integrity);
    init.set_signal(Some(&abort.signal()));
    let on_timeout = {
        let abort = abort.clone();
        Closure::<dyn FnMut()>::new(move || abort.abort())
    };
    let timer = window.set_timeout_with_callback_and_timeout_and_arguments_0(on_timeout.as_ref().unchecked_ref(), timeout_ms).map_err(js)?;
    let result = async {
        let response: web_sys::Response = JsFuture::from(window.fetch_with_str_and_init(url, &init)).await.map_err(js)?.dyn_into().map_err(js)?;
        if !response.ok() {
            return Err(format!("HTTP {} for {url}", response.status()));
        }
        let buffer = JsFuture::from(response.array_buffer().map_err(js)?).await.map_err(js)?;
        Ok(js_sys::Uint8Array::new(&buffer).to_vec())
    }
    .await;
    window.clear_timeout_with_handle(timer);
    // Kept alive until the timer can no longer fire.
    drop(on_timeout);
    result
}

/// Fetch a font on-demand from the server font proxy (e.g. `/fonts/{clean_family}/...`)
/// and register it in `FontDb::global()`. Request a repaint on `ctx` when loaded.
pub fn fetch_font_on_demand(family: &str, style: &str, ctx: egui::Context) {
    use std::sync::Mutex;
    static REQUESTED: std::sync::LazyLock<Mutex<std::collections::HashSet<String>>> =
        std::sync::LazyLock::new(|| Mutex::new(std::collections::HashSet::new()));

    let key = format!("{}:{}", family.to_lowercase(), style.to_lowercase());
    {
        let mut reqs = REQUESTED.lock().unwrap_or_else(|e| e.into_inner());
        if reqs.contains(&key) {
            return;
        }
        reqs.insert(key);
    }

    let fam = family.to_string();
    let sty = style.to_string();

    wasm_bindgen_futures::spawn_local(async move {
        let clean_fam = fam.to_lowercase().replace([' ', '-', '_'], "");
        let raw_fam = fam.replace(' ', "");
        let lower_fam = fam.to_lowercase().replace(' ', "-");
        let is_bold = sty.to_lowercase().contains("bold") || sty.contains("700");
        let is_italic = sty.to_lowercase().contains("italic");

        // Candidate URLs to query on the proxy
        let mut candidates = vec![];

        // 1. Google Fonts GitHub raw repo conventions
        if is_bold && is_italic {
            candidates.push(format!("/fonts/{clean_fam}/{raw_fam}-BoldItalic.ttf"));
            candidates.push(format!("/fonts/{clean_fam}/{raw_fam}-Italic[wght].ttf"));
            candidates.push(format!("/fonts/{clean_fam}/{raw_fam}-Italic[wdth,wght].ttf"));
        } else if is_bold {
            candidates.push(format!("/fonts/{clean_fam}/{raw_fam}-Bold.ttf"));
            candidates.push(format!("/fonts/{clean_fam}/{raw_fam}[wght].ttf"));
            candidates.push(format!("/fonts/{clean_fam}/{raw_fam}[wdth,wght].ttf"));
        } else if is_italic {
            candidates.push(format!("/fonts/{clean_fam}/{raw_fam}-Italic.ttf"));
            candidates.push(format!("/fonts/{clean_fam}/{raw_fam}-Italic[wght].ttf"));
            candidates.push(format!("/fonts/{clean_fam}/{raw_fam}-Italic[wdth,wght].ttf"));
        } else {
            candidates.push(format!("/fonts/{clean_fam}/{raw_fam}-Regular.ttf"));
            candidates.push(format!("/fonts/{clean_fam}/{raw_fam}[wght].ttf"));
            candidates.push(format!("/fonts/{clean_fam}/{raw_fam}[wdth,wght].ttf"));
            candidates.push(format!("/fonts/{clean_fam}/{raw_fam}[opsz,wght].ttf"));
        }
        // General fallback static filename
        candidates.push(format!("/fonts/{clean_fam}/{raw_fam}.ttf"));

        // 2. Fontsource files
        let weight_str = if is_bold { "700" } else { "400" };
        let style_str = if is_italic { "italic" } else { "normal" };
        candidates.push(format!("/fonts/{lower_fam}/files/{lower_fam}-latin-{weight_str}-{style_str}.woff2"));
        candidates.push(format!("/fonts/{lower_fam}/files/{lower_fam}-all-{weight_str}-{style_str}.woff"));

        let mut loaded = false;
        for url in candidates {
            match fetch_simple(&url, 15_000).await {
                Ok(bytes) => {
                    let added = FontDb::global().add_font(bytes);
                    if added > 0 {
                        log::info!("successfully loaded on-demand font '{fam}' from {url}");
                        loaded = true;
                        ctx.request_repaint();
                        break;
                    }
                }
                Err(_) => continue,
            }
        }

        if !loaded {
            log::warn!("could not fetch on-demand font '{fam}' '{sty}' from any proxy candidate URL");
        }
    });
}

/// GET `url` without SRI, aborted after `timeout_ms`.
async fn fetch_simple(url: &str, timeout_ms: i32) -> Result<Vec<u8>, String> {
    let window = web_sys::window().ok_or("no window")?;
    let abort = web_sys::AbortController::new().map_err(js)?;
    let init = web_sys::RequestInit::new();
    init.set_method("GET");
    init.set_signal(Some(&abort.signal()));
    let on_timeout = {
        let abort = abort.clone();
        Closure::<dyn FnMut()>::new(move || abort.abort())
    };
    let timer = window
        .set_timeout_with_callback_and_timeout_and_arguments_0(on_timeout.as_ref().unchecked_ref(), timeout_ms)
        .map_err(js)?;
    let result = async {
        let response: web_sys::Response =
            JsFuture::from(window.fetch_with_str_and_init(url, &init)).await.map_err(js)?.dyn_into().map_err(js)?;
        if !response.ok() {
            return Err(format!("HTTP {} for {url}", response.status()));
        }
        let buffer = JsFuture::from(response.array_buffer().map_err(js)?).await.map_err(js)?;
        Ok(js_sys::Uint8Array::new(&buffer).to_vec())
    }
    .await;
    window.clear_timeout_with_handle(timer);
    drop(on_timeout);
    result
}

fn js(e: wasm_bindgen::JsValue) -> String {
    format!("{e:?}")
}
