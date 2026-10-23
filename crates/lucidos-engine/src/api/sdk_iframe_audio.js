// Audio unlock shim — works on all browsers with autoplay restrictions.
// Monkey-patches AudioContext so app code reuses a shared, gesture-unlocked
// instance. Also unlocks HTML5 Audio. Persists listeners to re-unlock after
// iOS PWA background/foreground cycles.
(function() {
    var OrigAC = window.AudioContext || window.webkitAudioContext;
    if (!OrigAC) return;

    // Unlock by playing a silent buffer during a user gesture.
    // Not guarded by once — iOS PWA can re-suspend after backgrounding.
    function unlock() {
        var ctx = window._audioCtx;
        if (!ctx) {
            ctx = new OrigAC();
            window._audioCtx = ctx;
        }
        if (ctx.state === 'suspended') {
            ctx.resume().catch(function() {});
        }
        // Play a silent buffer to fully unlock the audio session
        try {
            var buf = ctx.createBuffer(1, 1, 22050);
            var src = ctx.createBufferSource();
            src.buffer = buf;
            src.connect(ctx.destination);
            src.start(0);
        } catch(e) {}
        // Also unlock HTML5 Audio (new Audio() / <audio> elements)
        try {
            var a = new Audio();
            a.src = 'data:audio/wav;base64,UklGRiQAAABXQVZFZm10IBAAAAABAAEAQB8AAIA+AAACABAAZGF0YQAAAAA=';
            a.volume = 0;
            a.play().then(function() { a.pause(); }).catch(function() {});
        } catch(e) {}
    }

    // Persistent listeners — check context state to avoid unnecessary work,
    // but don't remove listeners so we can re-unlock after PWA backgrounding.
    function tryUnlock() {
        if (!window._audioCtx || window._audioCtx.state !== 'running') {
            unlock();
        }
    }
    document.addEventListener('touchstart', tryUnlock, true);
    document.addEventListener('touchend', tryUnlock, true);
    document.addEventListener('click', tryUnlock, true);

    // When PWA returns from background, iOS suspends the AudioContext.
    // Mark it for re-unlock on next gesture, and try resume immediately.
    document.addEventListener('visibilitychange', function() {
        if (document.visibilityState === 'visible' && window._audioCtx) {
            window._audioCtx.resume().catch(function() {});
        }
    });

    // Monkey-patch AudioContext so app code reuses the shared instance.
    var PatchedAC = function AudioContext() {
        if (window._audioCtx) return window._audioCtx;
        var ctx = new OrigAC();
        window._audioCtx = ctx;
        return ctx;
    };
    PatchedAC.prototype = OrigAC.prototype;
    window.AudioContext = PatchedAC;
    if (window.webkitAudioContext) window.webkitAudioContext = PatchedAC;
})();
