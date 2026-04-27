use chromiumoxide::cdp::browser_protocol::input::{
    DispatchMouseEventParams, DispatchMouseEventType, MouseButton,
};
use chromiumoxide::cdp::browser_protocol::page::{FrameTree, GetFrameTreeParams};
use chromiumoxide::Page;
use std::time::Duration;

/// Auto-dismiss cookie consent dialogs.
/// Tries multiple strategies including CMP APIs and button clicking.
pub(super) async fn dismiss_cookie_consent(page: &Page) {
    log!("Starting cookie consent dismissal...");

    // JavaScript to find and click cookie consent buttons
    let consent_js = r#"
        (function() {
            // Only exact matches for accept-all type buttons
            const exactAcceptTexts = [
                // English - specific "accept all" variants
                'accept all', 'accept all cookies', 'allow all', 'allow all cookies',
                'allow essential and optional cookies', 'i agree', 'agree to all',
                // Norwegian
                'godta alle', 'godta alle cookies', 'aksepter alle', 'tillat alle',
                'jeg godtar', 'jeg aksepterer', 'godkjenn alle', 'ja, jeg samtykker',
                // Swedish
                'acceptera alla', 'godkänn alla',
                // Danish
                'accepter alle', 'tillad alle',
                // German
                'alle akzeptieren', 'alle cookies akzeptieren',
                // French
                'tout accepter', 'accepter tout',
                // Spanish
                'aceptar todo', 'aceptar todas',
                // Dutch
                'alles accepteren',
                // Italian
                'accetta tutto', 'accetta tutti'
            ];

            // Reject texts - never click these (be liberal)
            const rejectTexts = [
                'choose', 'customize', 'settings', 'preferences', 'manage', 'options',
                'reject', 'decline', 'deny', 'refuse', 'only essential', 'only necessary',
                'learn more', 'more info', 'read more', 'how we use', 'about', 'details',
                'privacy policy', 'cookie policy', 'purpose', 'partner', 'vendor',
                'category', 'categories',
                'velg', 'innstillinger', 'avvis', 'kun nødvendige', 'les mer', // Norwegian
                'ablehnen', 'einstellungen', // German
                'refuser', 'paramètres' // French
            ];

            // Find and click accept button
            const buttons = document.querySelectorAll('button, [role="button"], a.button, input[type="button"], input[type="submit"]');
            let fallbackButton = null;

            for (const btn of buttons) {
                const text = (btn.innerText || btn.textContent || btn.value || '').toLowerCase().trim();
                const rect = btn.getBoundingClientRect();

                // Size checks
                if (rect.width < 50 || rect.height < 20) continue;

                // Must be reasonably visible
                if (rect.top < 0 || rect.top > 800) continue;

                // Consent buttons are SHORT - reject long text
                if (text.length > 40) continue;

                // Skip if contains reject text
                if (rejectTexts.some(t => text.includes(t))) continue;

                // Match accept phrases - exact match preferred
                const normalizedText = text.replace(/[^a-z0-9æøåäöü\s]/g, '').trim();
                if (exactAcceptTexts.some(t => text === t || normalizedText === t)) {
                    console.log('[Lucidos] Clicking exact match: ' + text);
                    btn.click();
                    return 'clicked: ' + text;
                }

                // Also allow contains match for key phrases (only if text is short enough)
                if (text.length < 30) {
                    const keyPhrases = ['godta alle', 'accept all', 'allow all', 'aksepter alle', 'tillat alle'];
                    if (keyPhrases.some(p => text.includes(p) || normalizedText.includes(p))) {
                        console.log('[Lucidos] Clicking contains match: ' + text);
                        btn.click();
                        return 'clicked: ' + text;
                    }
                }

                // Track potential fallback (simple short accept words)
                if (!fallbackButton && ['accept', 'godta', 'accepter', 'aksepter'].includes(text)) {
                    fallbackButton = { btn, text };
                }
            }

            // Use fallback if no better match found
            if (fallbackButton) {
                console.log('[Lucidos] Clicking fallback: ' + fallbackButton.text);
                fallbackButton.btn.click();
                return 'clicked fallback: ' + fallbackButton.text;
            }

            // Return info about what we found
            const buttonTexts = Array.from(buttons).slice(0, 10).map(b => (b.innerText || '').substring(0, 40).replace(/\n/g, ' '));
            return 'not found. Buttons: ' + JSON.stringify(buttonTexts);
        })()
        "#;

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
                log!("Tried CMP API: {}", msg);
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
            log!("{}", msg);
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    // Try clicking in main frame
    for attempt in 1..=3 {
        if let Ok(result) = page.evaluate(consent_js).await {
            if let Ok(msg) = result.into_value::<String>() {
                log!("Consent search attempt {}: {}", attempt, msg);
                if msg.starts_with("clicked") {
                    log!("Cookie consent dismissed (attempt {}): {}", attempt, msg);
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
    let find_button_js = r#"
        (function() {
            // Check if an element or its ancestors contain cookie/consent context
            function hasConsentContext(el) {
                const contextWords = ['cookie', 'informasjonskapsl', 'personvern', 'privacy',
                    'consent', 'samtykke', 'gdpr', 'tracking', 'sporing'];
                let node = el;
                for (let i = 0; i < 6 && node && node !== document.body; i++) {
                    const t = (node.innerText || '').toLowerCase().substring(0, 500);
                    const id = (node.id || '').toLowerCase();
                    const cls = (node.className || '').toLowerCase();
                    if (contextWords.some(w => t.includes(w) || id.includes(w) || cls.includes(w))) return true;
                    node = node.parentElement;
                }
                return false;
            }

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
        })()
        "#;

    if let Ok(result) = page.evaluate(find_button_js).await {
        if let Ok(Some(json_str)) = result.into_value::<Option<String>>() {
            if let Ok(coords) = serde_json::from_str::<serde_json::Value>(&json_str) {
                if let (Some(x), Some(y)) = (coords["x"].as_f64(), coords["y"].as_f64()) {
                    let text = coords["text"].as_str().unwrap_or("unknown");
                    log!("Found consent button at ({}, {}): {}", x, y, text);

                    // Click at coordinates using CDP
                    if click_at_coordinates(page, x, y).await {
                        log!("Cookie consent dismissed via CDP click");
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

                // Check if this looks like a consent iframe
                const isConsentIframe =
                    src.includes('privacy') || src.includes('consent') || src.includes('sourcepoint') ||
                    src.includes('sp_message') || src.includes('gdpr') || src.includes('cmp.') ||
                    src.includes('/cmp') || src.includes('cookie') ||
                    id.includes('sp_message') || id.includes('consent') ||
                    className.includes('sp_message') || className.includes('consent');

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
                            src: src.substring(0, 100)
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
                    "Found consent iframe at ({}, {}) size {}x{}: {}",
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
                    log!("Trying CDP click at ({:.0}, {:.0})", click_x, click_y);
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
                            log!("Cookie consent iframe dismissed!");
                            tokio::time::sleep(Duration::from_millis(500)).await;
                            return;
                        }
                    }
                }

                log!("Clicked multiple positions but iframe still present");
            }
        }
    }

    // Try executing in all frames via CDP (including cross-origin iframes)
    // Retry a few times with increasing wait to allow iframes to load
    for attempt in 1..=3 {
        if attempt > 1 {
            log!("Waiting for iframes to load (attempt {})...", attempt);
            tokio::time::sleep(Duration::from_millis(1000)).await;
        }

        log!("Trying to find button in all frames via CDP...");
        if let Some((x, y, text)) = find_consent_button_in_frames(page).await {
            log!("Found consent button '{}' at ({}, {})", text, x, y);
            if click_at_coordinates(page, x, y).await {
                log!("Clicked consent button via CDP");
                tokio::time::sleep(Duration::from_millis(1000)).await;
                return;
            }
        }
    }

    log!("No cookie consent dialog found after all attempts");
}

/// Find consent button position by executing in all frames (including cross-origin)
async fn find_consent_button_in_frames(page: &Page) -> Option<(f64, f64, String)> {
    // Get all frames
    let frame_tree = page.execute(GetFrameTreeParams::default()).await.ok()?;

    // Collect all frame IDs and URLs, prioritizing consent-related frames
    let mut frame_ids = Vec::new();
    let mut frame_urls = Vec::new();
    collect_frame_ids_with_urls(&frame_tree.frame_tree, &mut frame_ids, &mut frame_urls);
    log!("Found {} frames to search", frame_ids.len());

    // Sort frames: consent-related iframes first, skip main frame (index 0)
    let mut frame_indices: Vec<usize> = (0..frame_ids.len()).collect();
    frame_indices.sort_by(|&a, &b| {
        let url_a = &frame_urls[a];
        let url_b = &frame_urls[b];
        let is_consent_a = url_a.contains("consent")
            || url_a.contains("privacy")
            || url_a.contains("cmp")
            || url_a.contains("cookie");
        let is_consent_b = url_b.contains("consent")
            || url_b.contains("privacy")
            || url_b.contains("cmp")
            || url_b.contains("cookie");
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

    // JavaScript to find button by searching for "Godta alle" etc. text
    // Searches ALL elements to find the accept button text
    let find_button_js = r#"
        (function() {
            // Key phrases to look for (must contain one of these)
            const acceptPhrases = [
                'godta alle', 'aksepter alle', 'tillat alle',
                'accept all', 'allow all', 'i agree',
                'acceptera alla', 'accepter alle',
                'alle akzeptieren', 'tout accepter',
                'aceptar todo', 'alles accepteren', 'accetta tutto'
            ];

            // Reject if contains these
            const rejectPhrases = [
                'settings', 'preferences', 'manage', 'reject', 'decline',
                'learn more', 'read more', 'how we use', 'privacy policy',
                'innstillinger', 'avvis', 'les mer', 'velg'
            ];

            // Search all elements for accept text
            const allElements = document.querySelectorAll('button, [role="button"], a, span, div, p');
            let bestMatch = null;

            for (const el of allElements) {
                const text = (el.innerText || el.textContent || '').toLowerCase().trim();

                // Skip if empty or too long
                if (!text || text.length > 50) continue;

                // Skip reject patterns
                if (rejectPhrases.some(p => text.includes(p))) continue;

                // Check for accept phrase
                const hasAccept = acceptPhrases.some(p => text.includes(p));
                if (!hasAccept) continue;

                const rect = el.getBoundingClientRect();

                // Must be visible
                if (rect.width < 20 || rect.height < 10) continue;
                if (rect.top < 0 || rect.top > 1400 || rect.left < 0) continue;

                // Prefer shorter text (more specific match)
                if (!bestMatch || text.length < bestMatch.textLen) {
                    bestMatch = {
                        x: rect.left + rect.width / 2,
                        y: rect.top + rect.height / 2,
                        text: text.substring(0, 50),
                        textLen: text.length
                    };
                }
            }

            if (bestMatch) {
                return JSON.stringify({
                    x: bestMatch.x + (window.screenX || 0),
                    y: bestMatch.y + (window.screenY || 0),
                    clientX: bestMatch.x,
                    clientY: bestMatch.y,
                    text: bestMatch.text
                });
            }

            // Debug: return info about what's in this frame
            const bodyText = (document.body?.innerText || '').substring(0, 200);
            const buttonCount = document.querySelectorAll('button').length;
            return JSON.stringify({
                debug: true,
                bodyPreview: bodyText.replace(/\n/g, ' ').substring(0, 100),
                buttonCount: buttonCount,
                url: location.href
            });
        })()
        "#;

    // Try each frame in priority order
    for &idx in &frame_indices {
        let frame_id = &frame_ids[idx];
        let frame_url = &frame_urls[idx];
        log!(
            "Searching frame {}: {}",
            idx,
            if frame_url.len() > 60 {
                &frame_url[..frame_url.floor_char_boundary(60)]
            } else {
                frame_url
            }
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
                        .expression(find_button_js.to_string())
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
                                                let preview =
                                                    coords["bodyPreview"].as_str().unwrap_or("");
                                                let btn_count =
                                                    coords["buttonCount"].as_i64().unwrap_or(0);
                                                let url = coords["url"].as_str().unwrap_or("");
                                                log!(
                                                    "Frame {}: {} buttons, url={}, preview='{}'",
                                                    idx,
                                                    btn_count,
                                                    url,
                                                    preview
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
                                                    "Frame {}: FOUND '{}' at ({}, {})",
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
                                    log!("Frame {}: null result", idx);
                                }
                            }
                            Err(e) => {
                                log!("Frame {}: eval error: {}", idx, e);
                            }
                        }
                    }
                }
                Err(e) => {
                    log!("Frame {}: create world error: {}", idx, e);
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
