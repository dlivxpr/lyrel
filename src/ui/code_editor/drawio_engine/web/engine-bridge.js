/*
 * Glue between the fixed draw.io viewer and the shell iframe.
 *
 * The viewer is the genuine draw.io viewer bundle (shapes and stencils are
 * embedded in it). This file only constructs a read-only GraphViewer, switches
 * pages and changes zoom; it never writes a document, opens a URL, or reads a
 * path chosen by the diagram.
 */
(function () {
    "use strict";

    var ORIGIN = window.location.origin;
    var MAX_XML = 16 * 1024 * 1024;
    var post = window.parent.postMessage.bind(window.parent);
    var addListener = window.addEventListener.bind(window);
    var viewer = null;
    var loaded = false;
    var pendingPage = 0;

    function report(kind, payload) {
        try {
            post({ v: 1, kind: kind, page: payload.page, pages: payload.pages,
                zoom: payload.zoom, message: payload.message }, ORIGIN);
        } catch (error) {
            // Nothing else can report a failed parent post.
        }
    }

    function fail(message) {
        var text = typeof message === "string" ? message : String(message);
        report("failed", { message: text.replace(/[\u0000-\u001f\u007f]+/g, " ").slice(0, 240) });
    }

    function pageCount() {
        return viewer !== null && viewer.diagrams !== null && viewer.diagrams.length > 0
            ? viewer.diagrams.length : 1;
    }

    function currentPage() {
        if (viewer === null) {
            return 0;
        }
        var shown = typeof viewer.getShownPage === "function" ? viewer.getShownPage() : -1;
        if (!Number.isInteger(shown) || shown < 0) {
            shown = Number.isInteger(viewer.currentPage) ? viewer.currentPage : 0;
        }
        return Math.max(0, Math.min(pageCount() - 1, shown));
    }

    function currentZoom() {
        if (viewer === null || viewer.graph === null || viewer.graph.view === null) {
            return 1;
        }
        var scale = viewer.graph.view.scale;
        return Number.isFinite(scale) && scale > 0 ? scale : 1;
    }

    function reportState() {
        report("state", { page: currentPage(), pages: pageCount(), zoom: currentZoom() });
    }

    function resetViewer() {
        if (viewer !== null) {
            try {
                if (viewer.graph !== null && typeof viewer.graph.destroy === "function") {
                    viewer.graph.destroy();
                }
            } catch (error) {
                // A partially initialized viewer is still discarded below.
            }
            viewer = null;
        }
        var container = document.getElementById("graph");
        while (container.firstChild !== null) {
            container.removeChild(container.firstChild);
        }
        loaded = false;
    }

    function parseXml(xml) {
        var parsed = new DOMParser().parseFromString(xml, "application/xml");
        if (parsed.getElementsByTagName("parsererror").length > 0) {
            throw new Error("diagram is not well-formed XML");
        }
        var root = parsed.documentElement;
        if (root === null) {
            throw new Error("diagram has no root element");
        }
        if (root.nodeName !== "mxfile" && root.nodeName !== "mxGraphModel") {
            throw new Error("unsupported diagram root: " + root.nodeName);
        }
        return root;
    }

    function applyPage(page) {
        pendingPage = page;
        if (viewer === null || !loaded) {
            return;
        }
        var pages = pageCount();
        var target = Math.max(0, Math.min(pages - 1, page));
        if (typeof viewer.selectPage === "function" && pages > 1) {
            viewer.selectPage(target);
        }
        reportState();
    }

    function applyZoom(command) {
        if (viewer === null || viewer.graph === null || !loaded) {
            return;
        }
        try {
            if (command === "fit" && viewer.graph.initialViewState !== undefined) {
                var initial = viewer.graph.initialViewState;
                viewer.graph.view.scaleAndTranslate(initial.scale, initial.translate.x,
                    initial.translate.y);
            } else if (command === "zoomIn") {
                viewer.graph.zoomIn();
            } else if (command === "zoomOut") {
                viewer.graph.zoomOut();
            } else if (command === "actual") {
                viewer.graph.zoomActual();
            }
        } catch (error) {
            fail(error === null || error === undefined ? "zoom failed" : error.message);
            return;
        }
        reportState();
    }

    function load(xml, page, theme) {
        if (typeof xml !== "string" || xml.length === 0 || xml.length > MAX_XML) {
            fail("diagram exceeds the preview budget");
            return;
        }
        resetViewer();
        pendingPage = Math.max(0, page | 0);
        document.documentElement.style.colorScheme = theme === "dark" ? "dark" : "light";
        var container = document.getElementById("graph");
        try {
            var root = parseXml(xml);
            if (typeof GraphViewer !== "function") {
                throw new Error("draw.io viewer is unavailable");
            }
            viewer = new GraphViewer(container, root, {
                "auto-fit": true,
                "resize": true,
                "toolbar": "",
                "nav": false,
                "lightbox": false,
                "check-visible-state": false,
                "toolbar-nohide": true,
                "page": pendingPage,
                "dark-mode": theme === "dark",
                "border": 8
            });
            if (viewer.graph === null) {
                viewer = null;
                throw new Error("draw.io viewer did not render");
            }
            viewer.graph.setEnabled(false);
            viewer.graph.setCellsEditable(false);
            viewer.graph.setCellsMovable(false);
            viewer.graph.setCellsResizable(false);
            viewer.graph.setConnectable(false);
            viewer.graph.setDropEnabled(false);
            viewer.graph.setPanning(false);
            loaded = true;
            if (pendingPage > 0) {
                applyPage(pendingPage);
            } else {
                reportState();
            }
            if (typeof window.requestAnimationFrame === "function") {
                window.requestAnimationFrame(function () {
                    if (loaded) {
                        reportState();
                    }
                });
            }
        } catch (error) {
            resetViewer();
            fail(error === null || error === undefined ? "diagram load failed" : error.message);
        }
    }

    addListener("message", function (event) {
        if (event.source !== window.parent || event.origin !== ORIGIN) {
            return;
        }
        var message = event.data;
        if (!message || typeof message !== "object" || message.v !== 1) {
            return;
        }
        if (message.kind === "load" && typeof message.xml === "string" &&
            Number.isInteger(message.page)) {
            load(message.xml, message.page, message.theme === "dark" ? "dark" : "light");
        } else if (message.kind === "page" && Number.isInteger(message.page) &&
            message.page >= 0) {
            applyPage(message.page);
        } else if (message.kind === "fit" || message.kind === "zoomIn" ||
            message.kind === "zoomOut" || message.kind === "actual") {
            applyZoom(message.kind);
        }
    });

    window.__drawioReady = function () {
        // GraphViewer prototypes are defined by the bundle that just ran; the
        // edit link and relative image bases must not leave this origin.
        if (typeof GraphViewer === "function") {
            GraphViewer.prototype.editBlankUrl = ORIGIN + "/drawio/blocked";
            GraphViewer.prototype.imageBaseUrl = ORIGIN + "/drawio/blocked/";
        }
        report("boot", {});
    };

    if (window.__drawioBooted) {
        window.__drawioReady();
    }
})();
