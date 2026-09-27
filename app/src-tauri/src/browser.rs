//! The Nexus browser, living inside the main window rather than beside it.
//!
//! A second *window* was the easy way to show a mod page, and it was wrong:
//! browsing is part of using this program, not an errand it sends you on. So
//! the page is a second webview parented to the main window, positioned over
//! the Browse tab's content area, with our own toolbar drawn above it.
//!
//! Two webviews in one window is Tauri's `unstable` feature. That is the only
//! way to do this -- an `<iframe>` cannot show Nexus at all, because the site
//! sends `X-Frame-Options: SAMEORIGIN` and fronts everything with Cloudflare.
//!
//! # It is a separate webview, so the layout has to be told, not inferred
//!
//! The child webview is a native surface stacked over the page; it knows
//! nothing about our CSS and our CSS knows nothing about it. The front end
//! measures the box the browser should fill and passes it here, in logical
//! pixels, whenever it changes -- on tab switch, on window resize, on sidebar
//! changes. Nothing keeps them in step automatically.

use serde::Serialize;
use tauri::{
    Emitter, LogicalPosition, LogicalSize, Manager, WebviewUrl, Window,
};

/// The child webview's label. One browser, reused.
pub const LABEL: &str = "browser";

/// Only ever Nexus. A link that leaves is opened in the real browser instead,
/// because this window has the user's Nexus session in it and should not be
/// carrying it to wherever a mod description points.
const ALLOWED: [&str; 3] = [
    "https://www.nexusmods.com/",
    "https://next.nexusmods.com/",
    "https://users.nexusmods.com/",
];

fn permitted(url: &str) -> bool {
    ALLOWED.iter().any(|prefix| url.starts_with(prefix))
}

/// Where the browser should sit, in logical pixels within the window.
#[derive(Debug, Clone, Copy, Serialize, serde::Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    /// A box too small to show anything is how the browser is parked when the
    /// user is on another tab: `hide()` is not reliable across platforms for a
    /// child webview, but a one-pixel box offscreen always is.
    fn hidden() -> Rect {
        Rect { x: -4000.0, y: 0.0, width: 1.0, height: 1.0 }
    }
}

/// Show `url` in the browser, creating it the first time.
#[tauri::command]
pub async fn browser_show(
    window: Window,
    url: String,
    rect: Rect,
) -> Result<(), String> {
    if !permitted(&url) {
        return Err(format!("{url} is not a Nexus page"));
    }
    let parsed: tauri::Url = url.parse().map_err(|_| format!("{url} is not a URL"))?;

    if let Some(existing) = window.get_webview(LABEL) {
        existing
            .navigate(parsed)
            .map_err(|e| format!("could not open that page: {e}"))?;
        place(&window, rect)?;
        return Ok(());
    }

    // Asked once, here, because the webview is built once: the scripts and the
    // request filter are both installed at creation. Turning the setting off
    // takes effect on the next page opened after the browser is closed, which
    // is the honest thing a toolbar toggle could not promise anyway. Until now
    // this setting was written to disk and read by nobody, so blocking was
    // always on whatever Settings said.
    let blocking = super::settings_now().block_ads;

    let app = window.app_handle().clone();
    let mut builder = tauri::webview::WebviewBuilder::new(LABEL, WebviewUrl::External(parsed));
    if blocking {
        builder = builder.initialization_script(super::adblock::SCRIPT);
    }
    let builder = builder
        .on_navigation(move |url| {
            // A download handed to us here, rather than to whatever owns
            // `nxm://` system wide, which is what keeps another mod
            // manager working for every other game.
            if url.scheme() == "nxm" {
                let _ = app.emit("nxm-link", url.to_string());
                return false;
            }
            let text = url.to_string();
            if permitted(&text) {
                let _ = app.emit("browser-navigated", &text);
                return true;
            }
            // Somewhere else entirely: hand it to the real browser rather than
            // take a window with a live Nexus session there.
            let _ = app.emit("browser-external", &text);
            false
        });

    let view = window
        .add_child(
            builder,
            LogicalPosition::new(rect.x, rect.y),
            LogicalSize::new(rect.width.max(1.0), rect.height.max(1.0)),
        )
        .map_err(|e| format!("could not open the browser: {e}"))?;

    // Network-level blocking, which is the half that actually stops the video
    // adverts. Has to happen after the webview exists, because it hooks the
    // live WebView2 instance.
    if blocking {
        super::adblock::install(&view);
    }
    Ok(())
}

/// Move and resize the browser to match the space the UI has left for it.
#[tauri::command]
pub fn browser_layout(window: Window, rect: Rect) -> Result<(), String> {
    place(&window, rect)
}

fn place(window: &Window, rect: Rect) -> Result<(), String> {
    let Some(view) = window.get_webview(LABEL) else {
        return Ok(()); // nothing open yet; the next `show` will position it
    };
    view.set_position(LogicalPosition::new(rect.x, rect.y))
        .map_err(|e| e.to_string())?;
    view.set_size(LogicalSize::new(rect.width.max(1.0), rect.height.max(1.0)))
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Park the browser offscreen, keeping the page loaded.
#[tauri::command]
pub fn browser_hide(window: Window) -> Result<(), String> {
    place(&window, Rect::hidden())
}

/// Close it and forget the page.
#[tauri::command]
pub fn browser_close(window: Window) -> Result<(), String> {
    if let Some(view) = window.get_webview(LABEL) {
        if let Err(e) = view.close() {
            // A page that will not close must at least get out of the way. This
            // webview is stacked over the window, so a failed close that left it
            // where it was would sit on top of whatever the user switched to,
            // with no button anywhere able to move it.
            let _ = place(&window, Rect::hidden());
            return Err(format!("could not close that page: {e}"));
        }
    }
    Ok(())
}


/// Back, forward and reload, driven from our own toolbar.
///
/// Tauri exposes no history API for a webview, so these go through the page's
/// own `history`, which is what the browser's buttons do anyway.
#[tauri::command]
pub fn browser_go(window: Window, what: String) -> Result<(), String> {
    let Some(view) = window.get_webview(LABEL) else {
        return Err("the browser is not open".into());
    };
    let script = match what.as_str() {
        "back" => "history.back()",
        "forward" => "history.forward()",
        "reload" => "location.reload()",
        other => return Err(format!("{other} is not a thing to do")),
    };
    view.eval(script).map_err(|e| e.to_string())
}
