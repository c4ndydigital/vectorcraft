//! VectorCraft server library: legal page rendering, Umami injection, and resource monitoring.

use std::sync::atomic::AtomicU64;
use std::time::{Duration, Instant};

/// Helper to escape raw text for HTML embedding.
pub fn html_escape(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for c in input.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

// ==============================================================================
// R9 & R10: Legal Pages & Umami Injection
// ==============================================================================

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct LegalConfig {
    pub company_or_name: String,
    pub address: String,
    pub email: String,
    pub phone: Option<String>,
    pub register_court: Option<String>,
    pub register_number: Option<String>,
    pub vat_id: Option<String>,
}

impl LegalConfig {
    pub fn from_env() -> Self {
        Self {
            company_or_name: std::env::var("LEGAL_COMPANY").or_else(|_| std::env::var("LEGAL_NAME")).unwrap_or_default(),
            address: std::env::var("LEGAL_ADDRESS").unwrap_or_default(),
            email: std::env::var("LEGAL_EMAIL").unwrap_or_default(),
            phone: std::env::var("LEGAL_PHONE").ok().filter(|s| !s.is_empty()),
            register_court: std::env::var("LEGAL_REGISTER_COURT").or_else(|_| std::env::var("LEGAL_REGISTER")).ok().filter(|s| !s.is_empty()),
            register_number: std::env::var("LEGAL_REGISTER_NUMBER").ok().filter(|s| !s.is_empty()),
            vat_id: std::env::var("LEGAL_VAT_ID").or_else(|_| std::env::var("LEGAL_TAX_ID")).ok().filter(|s| !s.is_empty()),
        }
    }

    pub fn is_configured(&self) -> bool {
        !self.company_or_name.is_empty() && !self.address.is_empty() && !self.email.is_empty()
    }
}

#[derive(Clone, Debug, Default)]
pub struct UmamiConfig {
    pub script_url: String,
    pub website_id: String,
}

impl UmamiConfig {
    pub fn from_env() -> Option<Self> {
        let script_url = std::env::var("UMAMI_SCRIPT_URL").ok()?;
        let website_id = std::env::var("UMAMI_WEBSITE_ID").ok()?;
        if script_url.is_empty() || website_id.is_empty() { None } else { Some(Self { script_url, website_id }) }
    }
}

/// Injects Umami analytics script tag before `</head>` if both env vars are present.
pub fn inject_umami(html: &str, umami: Option<&UmamiConfig>) -> String {
    let Some(u) = umami else { return html.to_string() };
    let script_tag =
        format!("  <script defer src=\"{}\" data-website-id=\"{}\"></script>\n</head>", html_escape(&u.script_url), html_escape(&u.website_id));
    html.replace("</head>", &script_tag)
}

/// Renders HTML for `/impressum` with dark theme matching VectorCraft.
pub fn render_impressum(config: &LegalConfig) -> String {
    let company_name =
        if config.company_or_name.is_empty() { "Dienstanbieter nicht konfiguriert (LEGAL_COMPANY fehlt)" } else { &config.company_or_name };

    let phone_row = if let Some(phone) = &config.phone { format!("<p><strong>Telefon:</strong> {}</p>", html_escape(phone)) } else { String::new() };

    let register_row = if let Some(court) = &config.register_court {
        let num = config.register_number.as_deref().unwrap_or("");
        format!(
            "<p><strong>Registereintrag:</strong> Eingetragen im Handelsregister.<br>Registergericht: {}<br>Registernummer: {}</p>",
            html_escape(court),
            html_escape(num)
        )
    } else {
        String::new()
    };

    let vat_row = if let Some(vat) = &config.vat_id {
        format!("<p><strong>Umsatzsteuer-Identifikationsnummer:</strong> {}</p>", html_escape(vat))
    } else {
        String::new()
    };

    format!(
        r#"<!DOCTYPE html>
<html lang="de">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Impressum – VectorCraft</title>
  <style>
    body {{
      font-family: system-ui, -apple-system, sans-serif;
      line-height: 1.6;
      background: #1e1e1e;
      color: #d4d4d4;
      margin: 0;
      padding: 32px 16px;
      display: flex;
      justify-content: center;
    }}
    .container {{
      max-width: 720px;
      width: 100%;
      background: #252526;
      padding: 32px;
      border-radius: 8px;
      border: 1px solid #333333;
    }}
    h1 {{ color: #ffffff; margin-top: 0; }}
    h2 {{ color: #e0e0e0; margin-top: 24px; }}
    a {{ color: #18a0fb; text-decoration: none; }}
    a:hover {{ text-decoration: underline; }}
    .footer-nav {{ margin-top: 32px; border-top: 1px solid #333; padding-top: 16px; }}
  </style>
</head>
<body>
  <div class="container">
    <h1>Impressum</h1>
    <p>Angaben gemäß § 5 TMG / § 18 MStV:</p>
    <h2>Diensteanbieter</h2>
    <p><strong>{}</strong><br>{}</p>
    <h2>Kontakt</h2>
    <p><strong>E-Mail:</strong> <a href="mailto:{}">{}</a></p>
    {}
    {}
    {}
    <h2>Haftung für Inhalte</h2>
    <p>Als Diensteanbieter sind wir gemäß § 7 Abs.1 TMG für eigene Inhalte auf diesen Seiten nach den allgemeinen Gesetzen verantwortlich. Nach §§ 8 bis 10 TMG sind wir jedoch nicht verpflichtet, übermittelte oder gespeicherte fremde Informationen zu überwachen.</p>
    <div class="footer-nav">
      <a href="/">&larr; Zurück zu VectorCraft</a> &bull; <a href="/datenschutz">Datenschutzerklärung</a>
    </div>
  </div>
</body>
</html>"#,
        html_escape(company_name),
        html_escape(&config.address).replace('\n', "<br>"),
        html_escape(&config.email),
        html_escape(&config.email),
        phone_row,
        register_row,
        vat_row
    )
}

/// Renders HTML for `/datenschutz` with dark theme matching VectorCraft.
pub fn render_datenschutz(config: &LegalConfig, umami: Option<&UmamiConfig>) -> String {
    let company_name = if config.company_or_name.is_empty() { "Diensteanbieter (LEGAL_COMPANY nicht gesetzt)" } else { &config.company_or_name };

    let umami_section = if let Some(u) = umami {
        format!(
            r#"<h2>Web-Analyse (Umami Analytics)</h2>
<p>Diese Website verwendet die datenschutzfreundliche, selbst gehostete Open-Source-Webanalyse <strong>Umami</strong>.
Umami setzt <em>keine Cookies</em>, speichert keine personenbezogenen Daten und verfolgt Nutzer nicht über verschiedene Websites hinweg.
Alle Daten werden aggregiert verarbeitet, die IP-Adresse wird anonymisiert und nicht dauerhaft gespeichert.
Rechtsgrundlage ist unser berechtigtes Interesse an der technischen Optimierung und Stabilität unseres Angebots gemäß Art. 6 Abs. 1 lit. f DSGVO.
Analyse-Host: <code>{}</code>.</p>"#,
            html_escape(&u.script_url)
        )
    } else {
        String::new()
    };

    format!(
        r#"<!DOCTYPE html>
<html lang="de">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Datenschutzerklärung – VectorCraft</title>
  <style>
    body {{
      font-family: system-ui, -apple-system, sans-serif;
      line-height: 1.6;
      background: #1e1e1e;
      color: #d4d4d4;
      margin: 0;
      padding: 32px 16px;
      display: flex;
      justify-content: center;
    }}
    .container {{
      max-width: 720px;
      width: 100%;
      background: #252526;
      padding: 32px;
      border-radius: 8px;
      border: 1px solid #333333;
    }}
    h1 {{ color: #ffffff; margin-top: 0; }}
    h2 {{ color: #e0e0e0; margin-top: 24px; }}
    a {{ color: #18a0fb; text-decoration: none; }}
    a:hover {{ text-decoration: underline; }}
    code {{ background: #2d2d2d; padding: 2px 6px; border-radius: 4px; font-size: 0.9em; }}
    .footer-nav {{ margin-top: 32px; border-top: 1px solid #333; padding-top: 16px; }}
  </style>
</head>
<body>
  <div class="container">
    <h1>Datenschutzerklärung</h1>
    <h2>1. Verantwortlicher</h2>
    <p>Verantwortlicher für die Datenverarbeitung auf dieser Website im Sinne der Datenschutz-Grundverordnung (DSGVO):<br>
    <strong>{}</strong><br>
    {}<br>
    E-Mail: <a href="mailto:{}">{}</a></p>

    <h2>2. Bereitstellung der Website und Server-Logfiles</h2>
    <p>Beim Aufruf dieser Website erfasst der Webserver automatisch technische Informationen, die Ihr Browser übermittelt (IP-Adresse, Datum/Uhrzeit des Zugriffs, übertragene Datenmenge, HTTP-Statuscode, User-Agent). Diese Daten werden ausschließlich zur Sicherstellung des störungsfreien Betriebs und der Systemsicherheit verarbeitet (Art. 6 Abs. 1 lit. f DSGVO). Eine Zusammenführung mit anderen Datenquellen erfolgt nicht.</p>

    <h2>3. Echtzeit-Kollaboration (WebSockets & Ephemerer Speicher)</h2>
    <p>Wenn Sie an einem geteilten Dokument arbeiten, verbindet sich Ihr Browser über eine WebSocket-Verbindung mit unserem Server.
    Dokumentänderungen (CRDT-Deltas) und Mauszeigerpositionen werden <strong>ausschließlich flüchtig im Arbeitsspeicher (RAM)</strong> verarbeitet und in Echtzeit an die aktiven Teilnehmer desselben Raums weitergeleitet.
    Es erfolgt <strong>keine persistente Speicherung auf Festplatte oder in Datenbanken</strong>. Nach dem Verlassen aller Teilnehmer werden temporäre Raumdaten automatisch nach einer kurzen Nachlaufzeit vollständig aus dem Arbeitsspeicher gelöscht.</p>

    <h2>4. Lokale Speicherung im Browser (LocalStorage & Cache)</h2>
    <p>VectorCraft speichert Entwürfe, Einstellungen und gecachte WebAssembly-Artefakte direkt lokal auf Ihrem Endgerät im Browser-Speicher (LocalStorage / IndexedDB). Diese Daten verbleiben auf Ihrem Gerät und werden nicht an unsere Server übertragen.</p>

    {}

    <h2>5. Ihre Rechte</h2>
    <p>Sie haben das Recht auf Auskunft (Art. 15 DSGVO), Berichtigung (Art. 16 DSGVO), Löschung (Art. 17 DSGVO), Einschränkung der Verarbeitung (Art. 18 DSGVO) sowie Datenübertragbarkeit (Art. 20 DSGVO). Wenden Sie sich hierzu an die oben angegebene E-Mail-Adresse.</p>

    <div class="footer-nav">
      <a href="/">&larr; Zurück zu VectorCraft</a> &bull; <a href="/impressum">Impressum</a>
    </div>
  </div>
</body>
</html>"#,
        html_escape(company_name),
        html_escape(&config.address).replace('\n', "<br>"),
        html_escape(&config.email),
        html_escape(&config.email),
        umami_section
    )
}

// ==============================================================================
// R14: Resource Monitoring, Alerts & Protected Metrics
// ==============================================================================

/// Snapshot of server resource consumption.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct ResourceMetrics {
    pub process_rss_bytes: u64,
    pub cgroup_used_bytes: Option<u64>,
    pub cgroup_max_bytes: Option<u64>,
    pub memory_percent: f64,
    pub rooms_count: usize,
    pub max_rooms: usize,
    pub rooms_percent: f64,
    pub active_connections: usize,
    pub largest_room_bytes: usize,
    pub rejected_by_limit_count: u64,
    pub uptime_seconds: u64,
}

/// Reads Linux cgroup memory limits and current usage if available.
pub fn read_cgroup_memory() -> (Option<u64>, Option<u64>) {
    let used = std::fs::read_to_string("/sys/fs/cgroup/memory.current").ok().and_then(|s| s.trim().parse::<u64>().ok());
    let max = std::fs::read_to_string("/sys/fs/cgroup/memory.max").ok().and_then(|s| s.trim().parse::<u64>().ok());
    (used, max)
}

/// Reads process resident set size (RSS) in bytes from `/proc/self/statm`.
pub fn read_process_rss() -> u64 {
    std::fs::read_to_string("/proc/self/statm")
        .ok()
        .and_then(|content| {
            let mut parts = content.split_whitespace();
            let _size = parts.next()?;
            let resident_pages: u64 = parts.next()?.parse().ok()?;
            let page_size = 4096; // 4 KB standard page
            Some(resident_pages * page_size)
        })
        .unwrap_or(0)
}

/// Threshold check evaluator with cooldown state tracking.
pub struct AlertEvaluator {
    pub mem_threshold_percent: f64,
    pub rooms_threshold_percent: f64,
    pub cooldown: Duration,
    pub send_recovery: bool,
    pub last_alert_time: Option<Instant>,
    pub in_alert_state: bool,
}

impl AlertEvaluator {
    pub fn new(mem_pct: f64, rooms_pct: f64, cooldown_minutes: u64, send_recovery: bool) -> Self {
        Self {
            mem_threshold_percent: mem_pct,
            rooms_threshold_percent: rooms_pct,
            cooldown: Duration::from_secs(cooldown_minutes * 60),
            send_recovery,
            last_alert_time: None,
            in_alert_state: false,
        }
    }

    /// Evaluates metrics against thresholds, returning alert message if fired.
    pub fn evaluate(&mut self, metrics: &ResourceMetrics, now: Instant) -> Option<AlertEvent> {
        let mem_exceeded = metrics.memory_percent >= self.mem_threshold_percent;
        let rooms_exceeded = metrics.rooms_percent >= self.rooms_threshold_percent;

        if mem_exceeded || rooms_exceeded {
            let can_fire = match self.last_alert_time {
                None => true,
                Some(last) => now.duration_since(last) >= self.cooldown,
            };
            if can_fire {
                self.last_alert_time = Some(now);
                self.in_alert_state = true;
                return Some(AlertEvent::Critical {
                    mem_percent: metrics.memory_percent,
                    rooms_percent: metrics.rooms_percent,
                    rooms_count: metrics.rooms_count,
                    active_connections: metrics.active_connections,
                });
            }
        } else if self.in_alert_state {
            self.in_alert_state = false;
            self.last_alert_time = None;
            if self.send_recovery {
                return Some(AlertEvent::Recovery { mem_percent: metrics.memory_percent, rooms_percent: metrics.rooms_percent });
            }
        }

        None
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum AlertEvent {
    Critical { mem_percent: f64, rooms_percent: f64, rooms_count: usize, active_connections: usize },
    Recovery { mem_percent: f64, rooms_percent: f64 },
}

impl AlertEvent {
    pub fn format_german(&self) -> String {
        match self {
            Self::Critical { mem_percent, rooms_percent, rooms_count, active_connections } => {
                format!(
                    "ACHTUNG: VectorCraft Ressourcengrenze überschritten!\n\
                    - Speicherauslastung: {:.1}%\n\
                    - Raum-Kapazität: {:.1}% ({} Räume aktiv)\n\
                    - Aktive Verbindungen: {}\n\
                    Empfehlung: Bitte Raumgrenzen prüfen oder Container skalieren.",
                    mem_percent, rooms_percent, rooms_count, active_connections
                )
            }
            Self::Recovery { mem_percent, rooms_percent } => {
                format!(
                    "ENTWARNUNG: VectorCraft Ressourcennutzung hat sich normalisiert.\n\
                    - Speicherauslastung: {:.1}%\n\
                    - Raum-Kapazität: {:.1}%\n\
                    System befindet sich wieder im normalen Betriebsbereich.",
                    mem_percent, rooms_percent
                )
            }
        }
    }
}

/// Validates bearer token for `/metrics` or `/status` endpoint.
pub fn check_bearer_auth(auth_header: Option<&str>, expected_token: Option<&str>) -> AuthResult {
    let Some(expected) = expected_token.filter(|t| !t.is_empty()) else {
        // If METRICS_TOKEN is not configured, the endpoint is disabled
        return AuthResult::Disabled;
    };

    let Some(header) = auth_header else {
        return AuthResult::Unauthorized;
    };

    let prefix = "Bearer ";
    if let Some(token) = header.strip_prefix(prefix) {
        if token.trim() == expected.trim() {
            return AuthResult::Authorized;
        }
    }

    AuthResult::Unauthorized
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum AuthResult {
    Authorized,
    Unauthorized,
    Disabled,
}

// Global rejected-by-limit counter
// Global rejected-by-limit counter
pub static REJECTED_BY_LIMIT_COUNT: AtomicU64 = AtomicU64::new(0);

// ==============================================================================
// Self-Hosted DSGVO / GDPR Compliant Font CDN & Proxy Service
// ==============================================================================

/// Configuration for the font CDN cache and on-demand proxy service.
#[derive(Clone, Debug)]
pub struct FontCdnConfig {
    pub enabled: bool,
    pub cache_dir: std::path::PathBuf,
    pub upstream_base: String,
}

impl FontCdnConfig {
    pub fn from_env(dist_path: &std::path::Path) -> Self {
        let enabled = std::env::var("FONTS_PROXY_ENABLED")
            .map(|v| v != "0" && v != "false" && v != "no")
            .unwrap_or(true);
        let cache_dir = std::env::var("FONTS_CACHE_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| dist_path.join("fonts"));
        let upstream_base = std::env::var("FONTS_CDN_UPSTREAM")
            .unwrap_or_else(|_| "https://cdn.jsdelivr.net/gh/google/fonts@main".to_string());
        Self { enabled, cache_dir, upstream_base }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum FontError {
    #[error("invalid font path: {0}")]
    InvalidPath(String),
    #[error("font proxy is disabled")]
    ProxyDisabled,
    #[error("font not found on upstream CDN")]
    NotFound,
    #[error("network or fetch error: {0}")]
    FetchError(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Sanitizes relative font path, rejecting any directory traversal (`..`, absolute paths, illegal characters).
pub fn sanitize_font_path(path: &str) -> Result<String, FontError> {
    let clean = path.trim().trim_start_matches('/');
    if clean.is_empty() {
        return Err(FontError::InvalidPath("empty path".to_string()));
    }
    for seg in clean.split('/') {
        if seg == ".." || seg == "." || seg.contains('\\') || seg.is_empty() {
            return Err(FontError::InvalidPath(format!("illegal path segment `{seg}`")));
        }
    }
    Ok(clean.to_string())
}

/// Returns the MIME content type based on font file extension.
pub fn font_mime_type(path: &str) -> &'static str {
    if path.ends_with(".woff2") {
        "font/woff2"
    } else if path.ends_with(".woff") {
        "font/woff"
    } else if path.ends_with(".ttf") {
        "font/ttf"
    } else if path.ends_with(".otf") {
        "font/otf"
    } else if path.ends_with(".json") {
        "application/json"
    } else {
        "application/octet-stream"
    }
}

/// Counts total cached font files inside the cache directory.
pub fn count_cached_fonts(dir: &std::path::Path) -> usize {
    if !dir.exists() {
        return 0;
    }
    let mut count = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        if let Ok(entries) = std::fs::read_dir(current) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.is_file() {
                    let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
                    if ext == "woff2" || ext == "woff" || ext == "ttf" || ext == "otf" {
                        count += 1;
                    }
                }
            }
        }
    }
    count
}

/// Fetches a font from disk cache or on-demand from upstream CDN.
/// Returns `(bytes, is_hit)`.
pub async fn fetch_and_cache_font(
    config: &FontCdnConfig,
    dist_path: &std::path::Path,
    path: &str,
) -> Result<(Vec<u8>, bool), FontError> {
    let clean = sanitize_font_path(path)?;

    // 1. Check local cache dir
    let cached_file = config.cache_dir.join(&clean);
    if cached_file.is_file() {
        if let Ok(bytes) = tokio::fs::read(&cached_file).await {
            if !bytes.is_empty() {
                return Ok((bytes, true));
            }
        }
    }

    // 2. Check dist_path/fonts
    let dist_file = dist_path.join("fonts").join(&clean);
    if dist_file.is_file() {
        if let Ok(bytes) = tokio::fs::read(&dist_file).await {
            if !bytes.is_empty() {
                return Ok((bytes, true));
            }
        }
    }

    // 3. If proxy is disabled, return NotFound
    if !config.enabled {
        return Err(FontError::ProxyDisabled);
    }

    // 4. On-demand upstream fetch via server-side curl (100% DSGVO compliant: client never contacts Google/upstream)
    let mut candidate_urls = vec![
        format!("{}/{}", config.upstream_base.trim_end_matches('/'), clean),
        format!("https://cdn.jsdelivr.net/gh/google/fonts@main/ofl/{}", clean),
        format!("https://cdn.jsdelivr.net/gh/google/fonts@main/apache/{}", clean),
        format!("https://cdn.jsdelivr.net/gh/google/fonts@main/ufl/{}", clean),
        format!("https://cdn.jsdelivr.net/gh/google/fonts@main/{}", clean),
        format!("https://cdn.jsdelivr.net/npm/@fontsource/{}", clean),
        format!("https://raw.githubusercontent.com/google/fonts/main/ofl/{}", clean),
        format!("https://raw.githubusercontent.com/google/fonts/main/{}", clean),
    ];

    if let Some((family, file)) = clean.split_once('/') {
        if !file.starts_with("files/") {
            candidate_urls.push(format!("https://cdn.jsdelivr.net/npm/@fontsource/{}/files/{}", family, file));
        }
    }

    let tmp_dir = config.cache_dir.join(".tmp");
    let _ = tokio::fs::create_dir_all(&tmp_dir).await;
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp_file = tmp_dir.join(format!("fetch-{}-{}", std::process::id(), nonce));

    let mut fetched_bytes: Option<Vec<u8>> = None;

    for url in candidate_urls {
        let status = tokio::process::Command::new("curl")
            .args(["-sSL", "--fail", "--max-time", "15", &url, "-o"])
            .arg(&tmp_file)
            .status()
            .await;

        if let Ok(exit) = status {
            if exit.success() {
                if let Ok(bytes) = tokio::fs::read(&tmp_file).await {
                    if !bytes.is_empty() {
                        fetched_bytes = Some(bytes);
                        break;
                    }
                }
            }
        }
        let _ = tokio::fs::remove_file(&tmp_file).await;
    }

    let Some(bytes) = fetched_bytes else {
        let _ = tokio::fs::remove_file(&tmp_file).await;
        return Err(FontError::NotFound);
    };

    // Store into cache_dir
    if let Some(parent) = cached_file.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    if tokio::fs::rename(&tmp_file, &cached_file).await.is_err() {
        let _ = tokio::fs::write(&cached_file, &bytes).await;
    }

    Ok((bytes, false))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_html_escape() {
        assert_eq!(html_escape("<b>\"Hello\" & 'World'</b>"), "&lt;b&gt;&quot;Hello&quot; &amp; &#39;World&#39;&lt;/b&gt;");
    }

    #[test]
    fn test_legal_config_escapes_html() {
        let cfg = LegalConfig {
            company_or_name: "<script>alert(1)</script>".to_string(),
            address: "Musterstr 1\n12345 Berlin".to_string(),
            email: "test@example.com".to_string(),
            phone: Some("+49 123 456789".to_string()),
            register_court: Some("Amtsgericht & Co".to_string()),
            register_number: Some("HRB 12345".to_string()),
            vat_id: Some("DE123456789".to_string()),
        };

        let html = render_impressum(&cfg);
        assert!(!html.contains("<script>alert(1)</script>"));
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(html.contains("Amtsgericht &amp; Co"));
        assert!(html.contains("test@example.com"));
    }

    #[test]
    fn test_umami_injection() {
        let base_html = "<html><head><title>Test</title></head><body></body></html>";
        let umami =
            UmamiConfig { script_url: "https://example.com/script.js".to_string(), website_id: "00000000-0000-0000-0000-000000000000".to_string() };

        let injected = inject_umami(base_html, Some(&umami));
        assert!(injected.contains("https://example.com/script.js"));
        assert!(injected.contains("00000000-0000-0000-0000-000000000000"));
        assert!(injected.contains("data-website-id="));

        let untouched = inject_umami(base_html, None);
        assert_eq!(untouched, base_html);
    }

    #[test]
    fn test_datenschutz_conditional_umami() {
        let cfg = LegalConfig {
            company_or_name: "Example GmbH".to_string(),
            address: "Musterstr 1".to_string(),
            email: "test@example.com".to_string(),
            ..Default::default()
        };

        // Without Umami
        let html_no_umami = render_datenschutz(&cfg, None);
        assert!(!html_no_umami.contains("Umami"));

        // With Umami
        let umami =
            UmamiConfig { script_url: "https://example.com/umami.js".to_string(), website_id: "00000000-0000-0000-0000-000000000000".to_string() };
        let html_with_umami = render_datenschutz(&cfg, Some(&umami));
        assert!(html_with_umami.contains("Umami Analytics"));
        assert!(html_with_umami.contains("https://example.com/umami.js"));
    }

    #[test]
    fn test_alert_threshold_evaluator_cooldown() {
        let mut evaluator = AlertEvaluator::new(80.0, 80.0, 60, true);
        let start = Instant::now();

        let mut metrics = ResourceMetrics { memory_percent: 85.0, rooms_percent: 50.0, rooms_count: 5, active_connections: 10, ..Default::default() };

        // Initial alert triggers
        let alert1 = evaluator.evaluate(&metrics, start);
        assert!(matches!(alert1, Some(AlertEvent::Critical { .. })));

        // 10 minutes later, still high -> should be silenced by cooldown
        let alert2 = evaluator.evaluate(&metrics, start + Duration::from_secs(600));
        assert!(alert2.is_none());

        // 65 minutes later, still high -> cooldown expired, triggers again
        let alert3 = evaluator.evaluate(&metrics, start + Duration::from_secs(3900));
        assert!(matches!(alert3, Some(AlertEvent::Critical { .. })));

        // Memory drops back to normal -> recovery triggers
        metrics.memory_percent = 50.0;
        let recovery = evaluator.evaluate(&metrics, start + Duration::from_secs(4000));
        assert!(matches!(recovery, Some(AlertEvent::Recovery { .. })));

        // Subsequent normal reading -> no alert
        let none = evaluator.evaluate(&metrics, start + Duration::from_secs(4100));
        assert!(none.is_none());
    }

    #[test]
    fn test_metrics_auth_checks() {
        // Disabled when METRICS_TOKEN is not configured
        assert_eq!(check_bearer_auth(None, None), AuthResult::Disabled);
        assert_eq!(check_bearer_auth(Some("Bearer secret"), None), AuthResult::Disabled);
        assert_eq!(check_bearer_auth(Some("Bearer secret"), Some("")), AuthResult::Disabled);

        // Unauthorized when missing or wrong
        assert_eq!(check_bearer_auth(None, Some("secret123")), AuthResult::Unauthorized);
        assert_eq!(check_bearer_auth(Some("Bearer wrong"), Some("secret123")), AuthResult::Unauthorized);
        assert_eq!(check_bearer_auth(Some("Basic abc"), Some("secret123")), AuthResult::Unauthorized);

        // Authorized when token matches
        assert_eq!(check_bearer_auth(Some("Bearer secret123"), Some("secret123")), AuthResult::Authorized);
    }

    #[test]
    fn test_sanitize_font_path() {
        assert_eq!(sanitize_font_path("inter/inter-latin-400-normal.woff2").unwrap(), "inter/inter-latin-400-normal.woff2");
        assert_eq!(sanitize_font_path("/roboto/Roboto-Regular.ttf").unwrap(), "roboto/Roboto-Regular.ttf");
        assert!(sanitize_font_path("../secret.txt").is_err());
        assert!(sanitize_font_path("inter/../../etc/passwd").is_err());
        assert!(sanitize_font_path("").is_err());
        assert!(sanitize_font_path("   ").is_err());
        assert!(sanitize_font_path("foo\\bar").is_err());
    }

    #[test]
    fn test_font_mime_type() {
        assert_eq!(font_mime_type("font.woff2"), "font/woff2");
        assert_eq!(font_mime_type("font.woff"), "font/woff");
        assert_eq!(font_mime_type("font.ttf"), "font/ttf");
        assert_eq!(font_mime_type("font.otf"), "font/otf");
        assert_eq!(font_mime_type("metadata.json"), "application/json");
        assert_eq!(font_mime_type("other.bin"), "application/octet-stream");
    }

    #[test]
    fn test_count_cached_fonts() {
        let temp_dir = std::env::temp_dir().join(format!("test-fonts-count-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp_dir);
        let sub = temp_dir.join("sub");
        std::fs::create_dir_all(&sub).unwrap();

        std::fs::write(temp_dir.join("a.woff2"), b"dummy").unwrap();
        std::fs::write(sub.join("b.ttf"), b"dummy").unwrap();
        std::fs::write(sub.join("c.txt"), b"dummy").unwrap(); // not a font

        assert_eq!(count_cached_fonts(&temp_dir), 2);
        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}

