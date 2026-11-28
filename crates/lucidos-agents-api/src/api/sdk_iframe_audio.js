(function() {
    var OrigAC = window.AudioContext || window.webkitAudioContext;
    if (!OrigAC) return;

    function unlock() {
        var ctx = window._audioCtx;
        if (!ctx) {
            ctx = new OrigAC();
            window._audioCtx = ctx;
        }
        if (ctx.state === 'suspended') {
            ctx.resume().catch(function() {});
        }
        try {
            var buf = ctx.createBuffer(1, 1, 22050);
            var src = ctx.createBufferSource();
            src.buffer = buf;
            src.connect(ctx.destination);
            src.start(0);
        } catch(e) {}
        try {
            var a = new Audio();
            a.src = 'data:audio/wav;base64,UklGRiQAAABXQVZFZm10IBAAAAABAAEAQB8AAIA+AAACABAAZGF0YQAAAAA=';
            a.volume = 0;
            a.play().then(function() { a.pause(); }).catch(function() {});
        } catch(e) {}
    }

    function tryUnlock() {
        if (!window._audioCtx || window._audioCtx.state !== 'running') {
            unlock();
        }
    }
    document.addEventListener('touchstart', tryUnlock, true);
    document.addEventListener('touchend', tryUnlock, true);
    document.addEventListener('click', tryUnlock, true);

    document.addEventListener('visibilitychange', function() {
        if (document.visibilityState === 'visible' && window._audioCtx) {
            window._audioCtx.resume().catch(function() {});
        }
    });

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
