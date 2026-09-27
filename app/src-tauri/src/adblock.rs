//! Keeping advertising out of the built-in browser.
//!
//! Two layers, because neither is sufficient alone.
//!
//! [`SCRIPT`] runs before any of the page's own code and is *cosmetic*: it
//! hides and removes ad slots. That alone would still let the page fetch every
//! ad, play video in the background and report impressions -- it would just
//! look clean while doing it.
//!
//! [`install`] is the real one. It asks WebView2 to route every network
//! request past us and refuses the ones aimed at ad and tracking hosts, so
//! nothing is fetched at all. That is a Windows-only API, which is why the
//! cosmetic layer exists as well: on any other platform it is all there is.
//!
//! # What is blocked, and what deliberately is not
//!
//! Only advertising and analytics hosts. Nexus's own domains, its image CDN
//! and Cloudflare's challenge scripts are all left alone -- blocking those
//! would break the site, and a browser that cannot show a mod page is worse
//! than one that shows an advert.

/// Hosts never allowed to load. Matched against the request's host, as a
/// suffix, so `doubleclick.net` also covers `stats.g.doubleclick.net`.
pub const BLOCKED_HOSTS: &[&str] = &[
    // Google's advertising stack
    "doubleclick.net",
    "googlesyndication.com",
    "googleadservices.com",
    "googletagservices.com",
    "googletagmanager.com",
    "google-analytics.com",
    "analytics.google.com",
    "adservice.google.com",
    // The networks Nexus itself runs
    "venatusmedia.com",
    "vmg.host",
    "playwire.com",
    "intergient.com",
    "vdo.ai",
    "vidcrunch.com",
    // General programmatic advertising
    "adnxs.com",
    "rubiconproject.com",
    "pubmatic.com",
    "openx.net",
    "criteo.com",
    "criteo.net",
    "taboola.com",
    "outbrain.com",
    "amazon-adsystem.com",
    "casalemedia.com",
    "sharethrough.com",
    "smartadserver.com",
    "teads.tv",
    "media.net",
    "33across.com",
    "sonobi.com",
    "indexww.com",
    "bidswitch.net",
    "adsrvr.org",
    "id5-sync.com",
    "crwdcntrl.net",
    "scorecardresearch.com",
    "quantserve.com",
    "moatads.com",
    // Session recording and product analytics
    "hotjar.com",
    "fullstory.com",
    "mixpanel.com",
    "segment.io",
    "segment.com",
    "sentry.io",
];

/// True when a URL's host is one we refuse to fetch.
///
/// Compares against the host only. A path containing "ads" is not an advert,
/// and a mod called "Taboola" should still be reachable.
pub fn blocked(url: &str) -> bool {
    let Some(host) = host_of(url) else {
        return false;
    };
    BLOCKED_HOSTS.iter().any(|bad| {
        host == *bad || host.ends_with(&format!(".{bad}"))
    })
}

/// The host part of an absolute URL, lowercased, without port or credentials.
fn host_of(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let authority = rest.split(['/', '?', '#']).next()?;
    // `user:pass@host:port` -> `host`
    let after_at = authority.rsplit('@').next()?;
    let host = after_at.split(':').next()?;
    (!host.is_empty()).then(|| host.to_lowercase())
}

/// Injected before the page's own scripts run.
///
/// Deliberately conservative about *what* it hides: it targets the containers
/// the ad networks create, not anything by position or size, so a wide banner
/// that happens to be a mod's own artwork survives.
pub const SCRIPT: &str = r#"
(function () {
  var HIDE = [
    '[id^="google_ads"]','[id^="div-gpt-ad"]','iframe[src*="doubleclick"]',
    'iframe[src*="googlesyndication"]','iframe[src*="venatus"]','iframe[src*="playwire"]',
    'iframe[src*="vdo.ai"]','ins.adsbygoogle','[class*="vm-placement"]','[id*="vm-av"]',
    '[id^="vdo-ai"]','[class^="adsbox"]','[data-ad-slot]','[data-google-query-id]',
    '#leaderboard','.adhesion','.ad-container','.ad-wrapper','[aria-label="Advertisement"]'
  ].join(',');

  function css() {
    var style = document.createElement('style');
    style.textContent = HIDE + '{display:none!important;height:0!important;}';
    (document.head || document.documentElement).appendChild(style);
  }
  if (document.head) { css(); }
  else { new MutationObserver(function (_, o) {
    if (document.head) { css(); o.disconnect(); }
  }).observe(document.documentElement, { childList: true, subtree: true }); }

  // The sticky video player is appended late and is not always inside a slot
  // we can name, so it is caught on the way in.
  new MutationObserver(function (records) {
    for (var r = 0; r < records.length; r++) {
      var added = records[r].addedNodes;
      for (var i = 0; i < added.length; i++) {
        var el = added[i];
        if (el.nodeType !== 1) continue;
        try { if (el.matches && el.matches(HIDE)) { el.remove(); continue; } } catch (e) {}
        var src = el.getAttribute && (el.getAttribute('src') || '');
        if (src && /doubleclick|googlesyndication|venatus|playwire|vdo\.ai|vidcrunch/.test(src)) {
          el.remove();
        }
      }
    }
  }).observe(document.documentElement, { childList: true, subtree: true });

  // Some players start audio before anything is visible.
  document.addEventListener('play', function (e) {
    var el = e.target;
    if (el && el.closest && el.closest(HIDE)) { el.pause(); }
  }, true);
})();
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ad_hosts_are_blocked_including_their_subdomains() {
        assert!(blocked("https://securepubads.g.doubleclick.net/tag/js/gpt.js"));
        assert!(blocked("https://doubleclick.net/x"));
        assert!(blocked("https://www.googletagmanager.com/gtm.js?id=X"));
        assert!(blocked("http://vdo.ai/player.js"));
    }

    #[test]
    fn nexus_and_its_cdn_are_never_blocked() {
        // Blocking any of these breaks the page, which is worse than an advert.
        assert!(!blocked("https://www.nexusmods.com/nomanssky/mods/3699"));
        assert!(!blocked("https://staticdelivery.nexusmods.com/mods/1634/images/x.jpg"));
        assert!(!blocked("https://challenges.cloudflare.com/turnstile/v0/api.js"));
        assert!(!blocked("https://users.nexusmods.com/auth/sign_in"));
    }

    #[test]
    fn a_blocked_name_inside_a_path_is_not_a_blocked_host() {
        // The host is what matters; a mod page about an ad network is fine.
        assert!(!blocked("https://www.nexusmods.com/search?q=doubleclick.net"));
        assert!(!blocked("https://www.nexusmods.com/taboola.com/mods/1"));
    }

    #[test]
    fn a_lookalike_domain_does_not_slip_through() {
        // `notdoubleclick.net` merely ends with the string; it is not a
        // subdomain of it, and a suffix test without the dot would pass it.
        assert!(!blocked("https://notdoubleclick.net/x"));
        assert!(!blocked("https://doubleclick.net.example.com/x"));
    }

    #[test]
    fn the_host_is_found_whatever_else_the_url_carries() {
        assert_eq!(host_of("https://A.B.COM:8443/x?y#z").as_deref(), Some("a.b.com"));
        assert_eq!(host_of("https://user:pw@host.com/x").as_deref(), Some("host.com"));
        assert_eq!(host_of("not a url"), None);
        assert_eq!(host_of("https://"), None);
    }

    #[test]
    fn the_injected_script_is_one_self_contained_expression() {
        // It is handed to the webview verbatim; an unbalanced brace here would
        // break every page in the browser rather than just the ad blocking.
        assert_eq!(
            SCRIPT.matches('(').count(),
            SCRIPT.matches(')').count(),
            "unbalanced parentheses"
        );
        assert_eq!(
            SCRIPT.matches('{').count(),
            SCRIPT.matches('}').count(),
            "unbalanced braces"
        );
    }
}

/// Refuse ad and tracker requests before they leave the machine.
///
/// WebView2 lets a host see every request the page makes and answer it
/// itself. Tauri does not surface that, so this reaches through to the
/// underlying `ICoreWebView2` and registers a `WebResourceRequested` handler:
/// anything aimed at a [`BLOCKED_HOSTS`] domain is answered with an empty
/// 403 and never reaches the network.
///
/// This is what actually stops the video adverts. The cosmetic [`SCRIPT`]
/// only hides what has already been fetched and started playing.
#[cfg(windows)]
pub fn install(webview: &tauri::Webview) {
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        ICoreWebView2_2, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
    };
    use webview2_com::WebResourceRequestedEventHandler;
    use windows::core::{Interface, HSTRING, PCWSTR};

    let outcome = webview.with_webview(|platform| unsafe {
        let Ok(core) = platform.controller().CoreWebView2() else {
            return;
        };

        // Every request, of every kind: scripts, images, XHR, media.
        let filter = HSTRING::from("*");
        if core
            .AddWebResourceRequestedFilter(
                PCWSTR(filter.as_ptr()),
                COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
            )
            .is_err()
        {
            return;
        }

        // The environment is needed to build the refusal, and only the `_2`
        // interface exposes it. An older WebView2 runtime without it simply
        // gets no blocking rather than a crash.
        let Ok(core2) = core.cast::<ICoreWebView2_2>() else {
            return;
        };
        let Ok(env) = core2.Environment() else {
            return;
        };

        let handler = WebResourceRequestedEventHandler::create(Box::new(move |_, args| {
            let Some(args) = args else {
                return Ok(());
            };
            let request = args.Request()?;
            let mut uri = windows::core::PWSTR::null();
            request.Uri(&mut uri)?;
            if uri.is_null() {
                return Ok(());
            }
            let url = uri.to_string().unwrap_or_default();
            windows::Win32::System::Com::CoTaskMemFree(Some(uri.0 as *const _));

            if !blocked(&url) {
                return Ok(());
            }
            // An empty 403 rather than an error: the page's own scripts cope
            // with an advert that failed to load, and fall over on some other
            // failures.
            let reason = HSTRING::from("Blocked");
            let headers = HSTRING::from("");
            if let Ok(response) = env.CreateWebResourceResponse(
                None,
                403,
                PCWSTR(reason.as_ptr()),
                PCWSTR(headers.as_ptr()),
            ) {
                let _ = args.SetResponse(&response);
            }
            Ok(())
        }));

        let mut token = Default::default();
        let _ = core.add_WebResourceRequested(&handler, &mut token);
    });

    if let Err(err) = outcome {
        // Not fatal: the page still loads, it just carries its advertising.
        eprintln!("ad blocking could not be installed: {err}");
    }
}

/// Nothing to hook into off Windows; the cosmetic layer is all there is.
#[cfg(not(windows))]
pub fn install(_webview: &tauri::Webview) {}
