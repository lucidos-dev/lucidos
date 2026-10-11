use chromiumoxide::cdp::browser_protocol::input::{
    DispatchMouseEventParams, DispatchMouseEventType, MouseButton,
};
use chromiumoxide::cdp::browser_protocol::page::{FrameTree, GetFrameTreeParams};
use chromiumoxide::Page;
use std::time::Duration;

/// `hasConsentContext`, the gate every consent click pass checks before it
/// clicks. The browser runs under the user's logged-in profile.
const CONSENT_CONTEXT_JS: &str = include_str!("browser_consent_context.js");

/// Clicks an accept button in the main frame.
const CONSENT_CLICK_JS: &str = include_str!("browser_consent_click.js");

/// Wraps a script `body` in an IIFE with `hasConsentContext` in scope.
fn consent_script(body: &str) -> String {
    format!("(function() {{\n{CONSENT_CONTEXT_JS}\n{body}\n}})()")
}

/// Auto-dismiss cookie consent dialogs.
/// Tries multiple strategies including CMP APIs and button clicking.
pub(super) async fn dismiss_cookie_consent(page: &Page) {
    log!("[BrowserConsent] Starting cookie consent dismissal...");

    let consent_js = consent_script(CONSENT_CLICK_JS);

    // Wait for cookie dialogs to appear (they often load async)
    tokio::time::sleep(Duration::from_millis(2000)).await;

    // Try CMP APIs directly from main frame
    // Note: We check if API exists but don't trust it actually dismisses the dialog
    // So we continue with other methods even if API is found
    let cmp_api_js = r#"
        (function() {
            let apiFound = null;

            // Sourcepoint - multiple API variants
            if (typeof window._sp_ !== 'undefined') {
                try {
                    if (window._sp_.acceptAll) {
                        window._sp_.acceptAll();
                        apiFound = 'Sourcepoint acceptAll';
                    } else if (window._sp_.gdpr && window._sp_.gdpr.acceptAll) {
                        window._sp_.gdpr.acceptAll();
                        apiFound = 'Sourcepoint gdpr.acceptAll';
                    }
                } catch(e) {}
            }

            // Note: TCF __tcfapi acceptAll sets consent but often doesn't close the dialog UI
            // Skipping this to avoid interference with button clicking

            // Sourcepoint via sp.consent
            if (!apiFound && typeof window.sp !== 'undefined' && window.sp.consent) {
                try {
                    window.sp.consent.acceptAll();
                    apiFound = 'sp.consent.acceptAll';
                } catch(e) {}
            }

            // Didomi
            if (!apiFound && typeof window.Didomi !== 'undefined') {
                try { window.Didomi.setUserAgreeToAll(); apiFound = 'Didomi'; } catch(e) {}
            }

            // OneTrust
            if (!apiFound && typeof window.OneTrust !== 'undefined') {
                try { window.OneTrust.AllowAll(); apiFound = 'OneTrust'; } catch(e) {}
            }

            // Cookiebot
            if (!apiFound && typeof window.Cookiebot !== 'undefined') {
                try { window.Cookiebot.submitCustomConsent(true, true, true); apiFound = 'Cookiebot'; } catch(e) {}
            }

            return apiFound || 'no API';
        })()
        "#;

    if let Ok(result) = page.evaluate(cmp_api_js).await {
        if let Ok(msg) = result.into_value::<String>() {
            if !msg.contains("no API") {
                log!("[BrowserConsent] Tried CMP API: {}", msg);
                tokio::time::sleep(Duration::from_millis(1000)).await;
                // Don't return - continue to verify with other methods
            }
        }
    }

    // Try Sourcepoint postMessage approach (works for privacy-mgmt.com iframes)
    let sourcepoint_postmessage_js = r#"
        (function() {
            // Find Sourcepoint iframes
            const iframes = document.querySelectorAll('iframe[src*="privacy-mgmt"], iframe[src*="sourcepoint"], iframe[src*="sp_message"]');
            for (const iframe of iframes) {
                try {
                    // Send acceptAll message to Sourcepoint iframe
                    iframe.contentWindow.postMessage({
                        __tcfapiCall: {
                            command: 'acceptAll',
                            version: 2,
                            callId: Date.now()
                        }
                    }, '*');

                    // Also try the sp_message format
                    iframe.contentWindow.postMessage({
                        type: 'sp.showMessage',
                        body: { type: 'accept' }
                    }, '*');

                    return 'sent postMessage to Sourcepoint iframe';
                } catch(e) {
                    console.log('postMessage error:', e);
                }
            }
            return null;
        })()
        "#;

    if let Ok(result) = page.evaluate(sourcepoint_postmessage_js).await {
        if let Ok(Some(msg)) = result.into_value::<Option<String>>() {
            log!("[BrowserConsent] {}", msg);
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    // Try clicking in main frame
    for attempt in 1..=3 {
        if let Ok(result) = page.evaluate(consent_js.as_str()).await {
            if let Ok(msg) = result.into_value::<String>() {
                log!(
                    "[BrowserConsent] Consent search attempt {}: {}",
                    attempt,
                    msg
                );
                if msg.starts_with("clicked") {
                    log!(
                        "[BrowserConsent] Cookie consent dismissed (attempt {}): {}",
                        attempt,
                        msg
                    );
                    // Wait longer for dialog to close
                    tokio::time::sleep(Duration::from_millis(1000)).await;
                    return;
                }
            }
        }
        // Wait between attempts
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    // Fallback: Try to find button position via JavaScript and click via CDP
    // This works for cross-origin iframes because we click at screen coordinates
    let find_button_js = consent_script(
        r#"
            const acceptPhrases = ['godta alle', 'accept all', 'aksepter alle',
                                   'tillat alle', 'allow all'];

            // Look for accept-like buttons in a consent context
            const buttons = Array.from(document.querySelectorAll('button, [role="button"]'));
            for (const btn of buttons) {
                const rect = btn.getBoundingClientRect();
                const style = window.getComputedStyle(btn);
                const text = (btn.innerText || '').toLowerCase();

                if (rect.width < 50 || rect.height < 20) continue;
                if (style.display === 'none' || style.visibility === 'hidden') continue;

                const hasAcceptText = acceptPhrases.some(p => text.includes(p));
                if (hasAcceptText && hasConsentContext(btn)) {
                    return JSON.stringify({
                        x: rect.left + rect.width / 2,
                        y: rect.top + rect.height / 2,
                        text: text.substring(0, 50)
                    });
                }
            }

            // Also check fixed/absolute positioned elements (likely modals)
            const allElements = document.querySelectorAll('*');
            for (const el of allElements) {
                const style = window.getComputedStyle(el);
                if (style.position === 'fixed' || style.position === 'absolute') {
                    if (parseInt(style.zIndex) > 1000) {
                        const btns = el.querySelectorAll('button');
                        for (const btn of btns) {
                            const rect = btn.getBoundingClientRect();
                            const text = (btn.innerText || '').toLowerCase();
                            if (rect.width > 50 && rect.height > 20) {
                                if (acceptPhrases.some(p => text.includes(p)) && hasConsentContext(el)) {
                                    return JSON.stringify({
                                        x: rect.left + rect.width / 2,
                                        y: rect.top + rect.height / 2,
                                        text: text.substring(0, 50)
                                    });
                                }
                            }
                        }
                    }
                }
            }

            return null;
        "#,
    );

    if let Ok(result) = page.evaluate(find_button_js).await {
        if let Ok(Some(json_str)) = result.into_value::<Option<String>>() {
            if let Ok(coords) = serde_json::from_str::<serde_json::Value>(&json_str) {
                if let (Some(x), Some(y)) = (coords["x"].as_f64(), coords["y"].as_f64()) {
                    let text = coords["text"].as_str().unwrap_or("unknown");
                    log!(
                        "[BrowserConsent] Found consent button at ({}, {}): {}",
                        x,
                        y,
                        text
                    );

                    // Click at coordinates using CDP
                    if click_at_coordinates(page, x, y).await {
                        log!("[BrowserConsent] Cookie consent dismissed via CDP click");
                        tokio::time::sleep(Duration::from_millis(500)).await;
                        return;
                    }
                }
            }
        }
    }

    // Last resort: Find consent iframes and click at typical button positions
    // Sourcepoint/CMP iframes often have the accept button in a predictable location
    let find_consent_iframe_js = r#"
        (function() {
            // Look for iframes that are likely consent dialogs
            const iframes = document.querySelectorAll('iframe');
            for (const iframe of iframes) {
                const src = iframe.src || '';
                const id = iframe.id || '';
                const className = iframe.className || '';
                let host = '';
                try { host = new URL(src).hostname; } catch (e) {}

                // Clicks land blind, at offsets measured on Sourcepoint's
                // dialog, so only a frame Sourcepoint marks as its own counts.
                // A keyword in some other frame's URL proves nothing.
                const isConsentIframe =
                    host.endsWith('privacy-mgmt.com') || host.includes('sourcepoint') ||
                    id.includes('sp_message') || className.includes('sp_message');

                if (isConsentIframe) {
                    const rect = iframe.getBoundingClientRect();
                    // Only if visible and reasonably sized
                    if (rect.width > 200 && rect.height > 100) {
                        // The "Accept all" button is typically in the right portion of the dialog
                        // Usually around 70-80% from the left, and 20-40% from the top
                        return JSON.stringify({
                            iframe_x: rect.left,
                            iframe_y: rect.top,
                            iframe_width: rect.width,
                            iframe_height: rect.height,
                            // Typical button position: right side of dialog, upper area
                            button_x: rect.left + rect.width * 0.75,
                            button_y: rect.top + rect.height * 0.25,
                            // Logged, so drop the query and fragment: they
                            // can carry an OAuth `code` or `access_token`.
                            src: src.split(/[?#]/)[0].substring(0, 100)
                        });
                    }
                }
            }
            return null;
        })()
        "#;

    if let Ok(result) = page.evaluate(find_consent_iframe_js).await {
        if let Ok(Some(json_str)) = result.into_value::<Option<String>>() {
            if let Ok(coords) = serde_json::from_str::<serde_json::Value>(&json_str) {
                let src = coords["src"].as_str().unwrap_or("unknown");
                let iframe_x = coords["iframe_x"].as_f64().unwrap_or(0.0);
                let iframe_y = coords["iframe_y"].as_f64().unwrap_or(0.0);
                let iframe_w = coords["iframe_width"].as_f64().unwrap_or(0.0);
                let iframe_h = coords["iframe_height"].as_f64().unwrap_or(0.0);

                log!(
                    "[BrowserConsent] Found consent iframe at ({}, {}) size {}x{}: {}",
                    iframe_x,
                    iframe_y,
                    iframe_w,
                    iframe_h,
                    src
                );

                // For fullscreen iframes, the actual modal is centered on screen
                // Based on actual button positions seen: Aftenposten button was at (1502, 1203) for 2560x1600 viewport
                // That's center (1280, 800) + offset (222, 403)
                let is_fullscreen = iframe_w > 2000.0 && iframe_h > 1000.0;

                let click_positions: Vec<(f64, f64)> = if is_fullscreen {
                    // Fullscreen overlay - Sourcepoint modal has button in bottom-right area
                    let center_x = iframe_x + iframe_w / 2.0;
                    let center_y = iframe_y + iframe_h / 2.0;
                    vec![
                        // Based on observed Aftenposten button at (1502, 1203) = center + (222, 403)
                        (center_x + 220.0, center_y + 400.0), // Exact position from Aftenposten
                        (center_x + 200.0, center_y + 380.0), // Slightly adjusted
                        (center_x + 250.0, center_y + 420.0), // More right, lower
                        (center_x + 180.0, center_y + 350.0), // Less offset
                        (center_x + 150.0, center_y + 300.0), // Even less
                        // Also try positions for different modal layouts
                        (center_x + 100.0, center_y + 250.0),
                        (center_x + 220.0, center_y + 300.0),
                        (center_x + 280.0, center_y + 400.0),
                        // Try left side too (some modals have button on left)
                        (center_x - 200.0, center_y + 400.0),
                        (center_x - 150.0, center_y + 350.0),
                    ]
                } else {
                    // Regular sized iframe - use ratio-based positions
                    vec![
                        (iframe_x + iframe_w * 0.75, iframe_y + iframe_h * 0.75),
                        (iframe_x + iframe_w * 0.70, iframe_y + iframe_h * 0.70),
                        (iframe_x + iframe_w * 0.80, iframe_y + iframe_h * 0.80),
                        (iframe_x + iframe_w * 0.65, iframe_y + iframe_h * 0.65),
                        (iframe_x + iframe_w * 0.50, iframe_y + iframe_h * 0.75),
                    ]
                };

                for (click_x, click_y) in click_positions {
                    log!(
                        "[BrowserConsent] Trying CDP click at ({:.0}, {:.0})",
                        click_x,
                        click_y
                    );
                    click_at_coordinates(page, click_x, click_y).await;
                    tokio::time::sleep(Duration::from_millis(300)).await;

                    // Check if iframe is still there
                    let check_js = r#"
                            document.querySelectorAll('iframe').length === 0 ||
                            !Array.from(document.querySelectorAll('iframe')).some(f =>
                                f.src && (f.src.includes('cmp') || f.src.includes('consent') || f.src.includes('privacy')))
                        "#;
                    if let Ok(result) = page.evaluate(check_js).await {
                        if let Ok(true) = result.into_value::<bool>() {
                            log!("[BrowserConsent] Cookie consent iframe dismissed!");
                            tokio::time::sleep(Duration::from_millis(500)).await;
                            return;
                        }
                    }
                }

                log!("[BrowserConsent] Clicked multiple positions but iframe still present");
            }
        }
    }

    // Try executing in all frames via CDP (including cross-origin iframes)
    // Retry a few times with increasing wait to allow iframes to load
    for attempt in 1..=3 {
        if attempt > 1 {
            log!(
                "[BrowserConsent] Waiting for iframes to load (attempt {})...",
                attempt
            );
            tokio::time::sleep(Duration::from_millis(1000)).await;
        }

        log!("[BrowserConsent] Trying to find button in all frames via CDP...");
        if let Some((x, y, text)) = find_consent_button_in_frames(page).await {
            log!(
                "[BrowserConsent] Found consent button '{}' at ({}, {})",
                text,
                x,
                y
            );
            if click_at_coordinates(page, x, y).await {
                log!("[BrowserConsent] Clicked consent button via CDP");
                tokio::time::sleep(Duration::from_millis(1000)).await;
                return;
            }
        }
    }

    log!("[BrowserConsent] No cookie consent dialog found after all attempts");
}

/// Finds the accept button inside ONE frame, by matching phrases inside a
/// consent dialog.
///
/// Module-level so `frame_consent_button_js_reports_no_page_text` can assert
/// the debug payload carries no page text. That payload is logged, and a
/// consent or callback page shows one-time codes in its visible body.
const FRAME_CONSENT_BUTTON_JS: &str = include_str!("browser_consent_frame_probe.js");

/// The origin and path of `url`. The query string and fragment can carry an
/// OAuth `code`, `access_token` or `id_token`.
fn without_query(url: &str) -> &str {
    url.split(['?', '#']).next().unwrap_or(url)
}

/// A frame served from a consent manager's host is searched first. Only the
/// host counts: an embedded `/privacy-policy` page or a `cmpid=` parameter
/// names no consent manager.
fn is_consent_frame_url(url: &str) -> bool {
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return false;
    };
    let host = parsed.host_str().unwrap_or_default();
    ["consent", "privacy", "cmp", "cookie"]
        .iter()
        .any(|word| host.contains(word))
}

async fn find_consent_button_in_frames(page: &Page) -> Option<(f64, f64, String)> {
    // Get all frames
    let frame_tree = page.execute(GetFrameTreeParams::default()).await.ok()?;

    // Collect all frame IDs and URLs, prioritizing consent-related frames
    let mut frame_ids = Vec::new();
    let mut frame_urls = Vec::new();
    collect_frame_ids_with_urls(&frame_tree.frame_tree, &mut frame_ids, &mut frame_urls);
    log!(
        "[BrowserConsent] Found {} frames to search",
        frame_ids.len()
    );

    // Sort frames: consent-related iframes first, skip main frame (index 0)
    let mut frame_indices: Vec<usize> = (0..frame_ids.len()).collect();
    frame_indices.sort_by(|&a, &b| {
        let is_consent_a = is_consent_frame_url(&frame_urls[a]);
        let is_consent_b = is_consent_frame_url(&frame_urls[b]);
        // Consent frames first, then by original index (skip main frame by putting it last)
        match (is_consent_a, is_consent_b) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => {
                // Put main frame (index 0) last
                if a == 0 {
                    std::cmp::Ordering::Greater
                } else if b == 0 {
                    std::cmp::Ordering::Less
                } else {
                    a.cmp(&b)
                }
            }
        }
    });

    let probe_js = consent_script(FRAME_CONSENT_BUTTON_JS);

    // Try each frame in priority order
    for &idx in &frame_indices {
        let frame_id = &frame_ids[idx];
        let frame_url = &frame_urls[idx];
        // The 60-char cap is a length guard, not redaction: a short host leaves
        // a token well inside it. `without_query` is the redaction.
        let logged_url = without_query(frame_url);
        log!(
            "[BrowserConsent] Searching frame {}: {}",
            idx,
            &logged_url[..logged_url.floor_char_boundary(60)]
        );
        // Use Runtime.evaluate with the frame's context
        // We need to get the execution context for this frame
        use chromiumoxide::cdp::browser_protocol::page::CreateIsolatedWorldParams;

        let create_world = CreateIsolatedWorldParams::builder()
            .frame_id(frame_id.clone())
            .world_name("consent-finder".to_string())
            .grant_univeral_access(true);

        if let Ok(params) = create_world.build() {
            match page.execute(params).await {
                Ok(world) => {
                    // Execute in this frame's context
                    use chromiumoxide::cdp::js_protocol::runtime::EvaluateParams;

                    let eval = EvaluateParams::builder()
                        .expression(probe_js.clone())
                        .context_id(world.execution_context_id);

                    if let Ok(eval_params) = eval.build() {
                        match page.execute(eval_params).await {
                            Ok(response) => {
                                if let Some(ref value) = response.result.result.value {
                                    if let Some(json_str) = value.as_str() {
                                        if let Ok(coords) =
                                            serde_json::from_str::<serde_json::Value>(json_str)
                                        {
                                            // Check if this is a debug response or actual button
                                            if coords.get("debug").is_some() {
                                                // Debug info about frame content
                                                let btn_count =
                                                    coords["buttonCount"].as_i64().unwrap_or(0);
                                                let url = coords["url"].as_str().unwrap_or("");
                                                log!(
                                                    "[BrowserConsent] Frame {}: {} buttons, url={}",
                                                    idx,
                                                    btn_count,
                                                    url
                                                );
                                            } else if let (Some(x), Some(y)) = (
                                                coords["clientX"].as_f64(),
                                                coords["clientY"].as_f64(),
                                            ) {
                                                let text = coords["text"]
                                                    .as_str()
                                                    .unwrap_or("")
                                                    .to_string();
                                                log!(
                                                    "[BrowserConsent] Frame {}: FOUND '{}' at ({}, {})",
                                                    idx,
                                                    text,
                                                    x,
                                                    y
                                                );
                                                return Some((x, y, text));
                                            }
                                        }
                                    }
                                } else {
                                    log!("[BrowserConsent] Frame {}: null result", idx);
                                }
                            }
                            Err(e) => {
                                log!("[BrowserConsent] Frame {}: eval error: {}", idx, e);
                            }
                        }
                    }
                }
                Err(e) => {
                    log!("[BrowserConsent] Frame {}: create world error: {}", idx, e);
                }
            }
        }
    }

    None
}

/// Helper to collect frame IDs and URLs from frame tree
fn collect_frame_ids_with_urls(
    tree: &FrameTree,
    ids: &mut Vec<chromiumoxide::cdp::browser_protocol::page::FrameId>,
    urls: &mut Vec<String>,
) {
    ids.push(tree.frame.id.clone());
    urls.push(tree.frame.url.clone());
    if let Some(ref children) = tree.child_frames {
        for child in children {
            collect_frame_ids_with_urls(child, ids, urls);
        }
    }
}

/// Click at specific screen coordinates using CDP
async fn click_at_coordinates(page: &Page, x: f64, y: f64) -> bool {
    // Mouse down
    let mouse_down = DispatchMouseEventParams::builder()
        .r#type(DispatchMouseEventType::MousePressed)
        .x(x)
        .y(y)
        .button(MouseButton::Left)
        .click_count(1);

    if let Ok(params) = mouse_down.build() {
        if page.execute(params).await.is_err() {
            return false;
        }
    }

    // Small delay
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Mouse up
    let mouse_up = DispatchMouseEventParams::builder()
        .r#type(DispatchMouseEventType::MouseReleased)
        .x(x)
        .y(y)
        .button(MouseButton::Left)
        .click_count(1);

    if let Ok(params) = mouse_up.build() {
        if page.execute(params).await.is_err() {
            return false;
        }
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The frame probe's debug payload is written to the engine log, so it
    /// must carry no page TEXT. A consent, callback or 2FA page shows a
    /// one-time passcode, a device code or the account email in its body.
    /// The log is persisted, and nothing redacts it afterwards.
    #[test]
    fn frame_consent_button_js_reports_no_page_text() {
        let debug_return = FRAME_CONSENT_BUTTON_JS
            .split("debug: true")
            .nth(1)
            .expect("the probe still returns a debug payload");

        assert!(
            !debug_return.contains("bodyPreview"),
            "the debug payload must not carry page text"
        );
        assert!(
            !debug_return.contains("innerText"),
            "the debug payload must not read the body's text"
        );
        assert!(
            debug_return.contains("buttonCount"),
            "the button count is the diagnostic and stays"
        );
    }

    /// The reported url is origin + pathname. `location.href` would carry the
    /// query and fragment, where an OAuth callback keeps `code`, `access_token`
    /// and `id_token`.
    #[test]
    fn frame_consent_button_js_logs_no_url_query() {
        let debug_return = FRAME_CONSENT_BUTTON_JS
            .split("debug: true")
            .nth(1)
            .expect("the probe still returns a debug payload");

        assert!(
            debug_return.contains("location.origin + location.pathname"),
            "the probe reports origin + pathname"
        );
        assert!(
            !debug_return.contains("location.href"),
            "location.href would carry an OAuth token into the log"
        );
    }

    /// A script must open with the IIFE. chromiumoxide sends a string that
    /// starts `function ` through `callFunctionOn`, which would call the
    /// context helper instead of running the body.
    #[test]
    fn consent_script_runs_the_body_with_the_context_helper_in_scope() {
        let script = consent_script("return 1;");

        assert!(script.starts_with("(function() {"));
        assert!(script.trim_end().ends_with("})()"));
        assert!(script.find("function hasConsentContext(el)") < script.find("return 1;"));
    }

    /// A path or a marketing parameter names no consent manager.
    #[test]
    fn consent_frame_url_reads_only_the_host() {
        assert!(!is_consent_frame_url(
            "https://signup.example.com/form?cmpid=123"
        ));
        assert!(!is_consent_frame_url("https://example.com/terms#cookie"));
        assert!(!is_consent_frame_url("https://example.com/privacy-policy"));
        assert!(!is_consent_frame_url("about:blank"));
        assert!(is_consent_frame_url(
            "https://cdn.privacy-mgmt.example.net/index.html?id=1"
        ));
    }
}
