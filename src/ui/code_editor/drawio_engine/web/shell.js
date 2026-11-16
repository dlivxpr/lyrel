/*
 * The only page the native host lets WebView2 navigate to.
 *
 * The draw.io viewer runs in a same-origin iframe with a minimal sandbox. This
 * page owns the native session token; the engine page never receives it. Every
 * message crossing a boundary is checked for its source, origin, session,
 * sequence number and shape before it is forwarded, and native commands use an
 * independent monotonic sequence.
 */
(function () {
    "use strict";

    var webview = window.chrome && window.chrome.webview ? window.chrome.webview : null;
    var ipc = window.ipc;
    var nativePost = webview && typeof webview.postMessage === "function"
        ? webview.postMessage.bind(webview)
        : (ipc && typeof ipc.postMessage === "function"
            ? ipc.postMessage.bind(ipc)
            : null);
    var nativeListen = webview && typeof webview.addEventListener === "function"
        ? webview.addEventListener.bind(webview)
        : null;
    var jsonParse = JSON.parse.bind(JSON);
    var jsonStringify = JSON.stringify.bind(JSON);
    var postMessage = window.postMessage.bind(window);
    var addWindowListener = window.addEventListener.bind(window);
    var fetchDocument = window.fetch.bind(window);
    var freeze = Object.freeze;
    var defineProperty = Object.defineProperty;

    var ORIGIN = window.location.origin;
    var ENGINE_PAGE = "/drawio/engine.html";
    var MAX_DOCUMENT = 16 * 1024 * 1024;
    var MAX_REPORT = 240;

    var token = null;
    var session = null;
    var documentPath = null;
    var theme = "light";
    var bound = false;
    var frame = null;
    var engineReady = false;
    var documentSent = false;
    var outboundSeq = 0;
    var lastNativeSeq = 0;
    var pendingPage = 0;
    var failed = false;

    function isHex32(value) {
        return typeof value === "string" && /^[0-9a-f]{32}$/.test(value);
    }

    function isDocumentPath(value) {
        return typeof value === "string" &&
            /^\/drawio\/session\/[0-9a-f]{32}\/document$/.test(value);
    }

    function shortText(value) {
        var text = typeof value === "string" ? value : String(value == null ? "unknown error" : value);
        text = text.replace(/[\u0000-\u001f\u007f]+/g, " ").replace(/\s+/g, " ").trim();
        return text.slice(0, MAX_REPORT);
    }

    function send(kind, payload) {
        if (!nativePost || token === null || session === null) {
            return;
        }
        outboundSeq += 1;
        try {
            nativePost(jsonStringify({
                v: 1,
                session: session,
                token: token,
                seq: outboundSeq,
                kind: kind,
                payload: payload || {}
            }));
        } catch (error) {
            // A failed native post cannot be repaired from this page.
        }
    }

    function fail(message) {
        if (failed) {
            return;
        }
        failed = true;
        send("failed", { message: shortText(message) });
    }

    function enginePage() {
        if (frame !== null) {
            return;
        }
        frame = document.createElement("iframe");
        frame.setAttribute("sandbox", "allow-scripts allow-same-origin");
        frame.setAttribute("allow",
            "camera 'none'; microphone 'none'; geolocation 'none'; " +
            "clipboard-read 'none'; clipboard-write 'none'; fullscreen 'none'; " +
            "autoplay 'none'");
        frame.setAttribute("referrerpolicy", "no-referrer");
        frame.setAttribute("title", "draw.io diagram");
        frame.src = ENGINE_PAGE;
        document.getElementById("frame-host").appendChild(frame);
    }

    function postToEngine(message) {
        if (frame === null || !engineReady) {
            return false;
        }
        try {
            frame.contentWindow.postMessage(message, ORIGIN);
            return true;
        } catch (error) {
            fail(error == null ? "engine message failed" : error.message);
            return false;
        }
    }

    function loadDocument() {
        if (documentSent || documentPath === null) {
            return;
        }
        documentSent = true;
        fetchDocument(documentPath, {
            method: "GET",
            credentials: "omit",
            cache: "no-store",
            redirect: "error",
            referrerPolicy: "no-referrer"
        }).then(function (response) {
            if (!response.ok) {
                throw new Error("diagram read failed with status " + response.status);
            }
            return response.text();
        }).then(function (text) {
            if (typeof text !== "string" || text.length === 0) {
                throw new Error("diagram is empty");
            }
            if (text.length > MAX_DOCUMENT) {
                throw new Error("diagram exceeds the preview budget");
            }
            if (!postToEngine({
                v: 1,
                kind: "load",
                xml: text,
                page: pendingPage,
                theme: theme,
                commit: documentPath.replace(/document$/, "commit"),
                sync: documentPath.replace(/document$/, "sync")
            })) {
                throw new Error("diagram engine is not ready");
            }
        }).catch(function (error) {
            fail(error == null ? "diagram load failed" : error.message);
        });
    }

    addWindowListener("message", function (event) {
        if (frame === null || event.source !== frame.contentWindow || event.origin !== ORIGIN) {
            return;
        }
        var message = event.data;
        if (!message || typeof message !== "object" || message.v !== 1) {
            return;
        }
        if (message.kind === "boot") {
            if (engineReady) {
                return;
            }
            engineReady = true;
            send("ready", {});
            loadDocument();
            return;
        }
        if (message.kind === "state") {
            var page = typeof message.page === "number" ? message.page : 0;
            var pages = typeof message.pages === "number" ? message.pages : 0;
            var zoom = typeof message.zoom === "number" ? message.zoom : 1;
            if (!Number.isInteger(page) || page < 0 || !Number.isInteger(pages) || pages < 0) {
                return;
            }
            if (!Number.isFinite(zoom) || zoom <= 0) {
                zoom = 1;
            }
            send("state", { page: page, pages: pages, zoom: zoom });
            return;
        }
        if (message.kind === "undo" || message.kind === "redo" ||
            message.kind === "save" || message.kind === "paste") {
            send(message.kind, {});
            return;
        }
        if (message.kind === "sync-done") {
            var request = typeof message.request === "number" &&
                Number.isInteger(message.request) && message.request > 0
                ? message.request : 0;
            if (request <= 0) {
                return;
            }
            send("sync-done", { request: request });
            return;
        }
        if (message.kind === "commit-failed") {
            var status = typeof message.status === "number" &&
                Number.isInteger(message.status) && message.status >= 0 &&
                message.status <= 599 ? message.status : 0;
            var reason = typeof message.message === "string" ? shortText(message.message) : "";
            if (message.sync === true) {
                var syncRequest = typeof message.request === "number" &&
                    Number.isInteger(message.request) && message.request > 0
                    ? message.request : 0;
                if (syncRequest <= 0) {
                    return;
                }
                send("commit-failed", {
                    sync: true,
                    request: syncRequest,
                    status: status,
                    message: reason
                });
            } else {
                send("commit-failed", {
                    sync: false,
                    status: status,
                    message: reason
                });
            }
            return;
        }
        if (message.kind === "clipboard" && typeof message.text === "string") {
            send("clipboard", { text: message.text, cut: message.cut === true });
            return;
        }
        if (message.kind === "failed") {
            fail(message.message);
        }
    });

    function handleNativeMessage(input) {
        var message = input;
        if (typeof message === "string") {
            try {
                message = jsonParse(message);
            } catch (error) {
                return;
            }
        }
        if (!message || typeof message !== "object" || message.v !== 1) {
            return;
        }
        if (session === null || message.session !== session) {
            return;
        }
        if (typeof message.seq !== "number" || !Number.isInteger(message.seq) ||
            message.seq <= lastNativeSeq) {
            return;
        }
        lastNativeSeq = message.seq;
        if (message.cmd === "page" && typeof message.page === "number" &&
            Number.isInteger(message.page) && message.page >= 0) {
            pendingPage = message.page;
            postToEngine({ v: 1, kind: "page", page: pendingPage });
        } else if (message.cmd === "fit" || message.cmd === "zoomIn" ||
            message.cmd === "zoomOut" || message.cmd === "actual" ||
            message.cmd === "sync" || message.cmd === "copy" ||
            message.cmd === "cut" || message.cmd === "delete") {
            postToEngine({ v: 1, kind: message.cmd, seq: message.seq });
        } else if (message.cmd === "paste" && typeof message.text === "string") {
            postToEngine({ v: 1, kind: "paste", text: message.text, seq: message.seq });
        }
    }

    if (nativeListen !== null) {
        nativeListen("message", function (event) {
            handleNativeMessage(event.data);
        });
    }

    function bind(payload) {
        if (bound || !payload || typeof payload !== "object") {
            return "invalid";
        }
        if (!isHex32(payload.token) || !isHex32(payload.session) ||
            !isDocumentPath(payload.document)) {
            return "invalid";
        }
        if (payload.theme !== "light" && payload.theme !== "dark") {
            return "invalid";
        }
        token = payload.token;
        session = payload.session;
        documentPath = payload.document;
        theme = payload.theme;
        bound = true;
        defineProperty(document.documentElement.style, "colorScheme", {
            value: theme === "dark" ? "dark" : "light",
            writable: true
        });
        enginePage();
        return "ok";
    }

    defineProperty(window, "__lyrel", {
        value: freeze({
            bind: bind,
            // macOS has no chrome.webview event channel; the native side
            // evaluates this function after the top frame finishes loading.
            nativeMessage: handleNativeMessage
        }),
        writable: false,
        configurable: false,
        enumerable: false
    });
})();
