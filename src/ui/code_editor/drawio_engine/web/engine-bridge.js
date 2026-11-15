/*
 * Glue between the fixed draw.io viewer bundle and the shell iframe.
 *
 * The bundle is the genuine draw.io engine: GraphViewer renders, the Editor it
 * owns drives the real mxUndoManager, mxCellEditor, mxGraphHandler and
 * mxRubberband. This file enables real model editing on that graph, reports
 * every committed edit to the workbench over the shell, and applies native
 * commands — page, zoom, undo, redo, clipboard and the save flush — to the
 * same engine. It never opens a URL, reads a path the diagram names, or keeps
 * a second document authority: the mxfile DOM this file maintains is only the
 * projection of the bytes the workbench delivered and every committed edit
 * travels back through the session.
 */
(function () {
    "use strict";

    var viewer = null;
    var fileRoot = null;
    var xmlPrefix = "";
    var loaded = false;
    var pendingPage = 0;
    var commitPath = null;
    var syncPath = null;
    var postFile = window.fetch.bind(window);
    // The document budget the engine accepts, and the one wire this page may
    // post across: every report goes to the shell through the parent window.
    var MAX_XML = 16 * 1024 * 1024;
    var post = window.parent.postMessage.bind(window.parent);
    var addListener = window.addEventListener.bind(window);
    // Same-origin gate for every message crossing the iframe boundary.
    var ORIGIN = window.location.origin;
    // Programmatic model changes that must not count as document edits: a
    // page switch stashing the previous page, a reload, or internal teardown.
    var suppress = 0;
    // The monotonic sequence a commit carries; the workbench refuses stale ones.
    var editSeq = 0;

    function report(kind, payload) {
        var data = payload || {};
        try {
            // Only these fields ever cross to the shell: a payload cannot
            // smuggle a key the native side does not expect.
            post({ v: 1, kind: kind, page: data.page, pages: data.pages,
                zoom: data.zoom, text: data.text, cut: data.cut,
                message: data.message }, ORIGIN);
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

    function graph() {
        return viewer !== null ? viewer.graph : null;
    }

    function diagramNode() {
        if (viewer === null || viewer.diagrams === null) {
            return null;
        }
        return viewer.diagrams[currentPage()] || null;
    }

    // Writes the live model back into the mxfile DOM the engine was loaded
    // from, keeping the page's original representation: a compressed page gets
    // fresh deflate-base64 content and an uncompressed page gets a fresh
    // mxGraphModel child; every other diagram node and every mxfile/diagram
    // attribute is left byte-identical.
    function writePageBack() {
        if (fileRoot === null || viewer === null || viewer.editor === null) {
            return;
        }
        var diagram = diagramNode();
        if (diagram === null) {
            return;
        }
        var model = viewer.editor.getGraphXml();
        var compressed = diagram.getElementsByTagName("mxGraphModel").length === 0;
        if (compressed) {
            while (diagram.firstChild !== null) {
                diagram.removeChild(diagram.firstChild);
            }
            diagram.appendChild(diagram.ownerDocument.createTextNode(
                Graph.compress(mxUtils.getXml(model))));
        } else {
            while (diagram.firstChild !== null) {
                diagram.removeChild(diagram.firstChild);
            }
            diagram.appendChild(diagram.ownerDocument.importNode(model, true));
        }
    }

    // The canonical mxfile bytes: the diagram nodes this file's edits touched
    // plus every untouched page, attribute and wrapper, in the same wrapper
    // form the workbench delivered.
    function serializeFile() {
        if (fileRoot === null) {
            return null;
        }
        return xmlPrefix + mxUtils.getXml(fileRoot);
    }

    // One edit, undo, redo or explicit sync becomes the document's newest
    // bytes. Commits travel over an HTTP POST to this session's own commit
    // path — never through postMessage, which the host caps far below the
    // document budget. A page the workbench replaced underneath us is refused
    // by the session's sequence check; the resync decides the content.
    function commitNow(sync) {
        if (suppress > 0 || !loaded || commitPath === null) {
            return;
        }
        writePageBack();
        var xml = serializeFile();
        if (typeof xml !== "string" || xml.length === 0) {
            return;
        }
        editSeq += 1;
        var path = (sync === true ? syncPath : commitPath) + "/" + editSeq;
        try {
            postFile(path, {
                method: "POST",
                credentials: "omit",
                cache: "no-store",
                redirect: "error",
                referrerPolicy: "no-referrer",
                body: xml
            }).catch(function () {
                // A refused commit is not fatal: the next edit, save sync or
                // reload re-offers the same in-memory document.
            });
        } catch (error) {
            // Same: the in-memory document still holds the edit.
        }
    }

    function commitLater() {
        commitNow(false);
    }

    // The engine's own selection encoded the way draw.io clipboard entries
    // carry cells, for the workbench's system clipboard.
    function selectionXml() {
        var g = graph();
        if (g === null) {
            return "";
        }
        var cells = g.getSelectionCells();
        if (cells === null || cells.length === 0) {
            return "";
        }
        var encoded = g.encodeCells(mxUtils.sortCells(
            g.model.getTopmostCells(cells)));
        var xml = mxUtils.getXml(encoded);
        return xml.indexOf("<mxGraphModel") === 0 ? xml : "";
    }

    function clipboardOut(cut) {
        var xml = selectionXml();
        if (xml.length === 0) {
            return;
        }
        report("clipboard", { text: xml, cut: cut === true });
    }

    function copyCells(cut) {
        var g = graph();
        if (g === null || viewer.editor === null) {
            return;
        }
        var cells = cut ? g.getDeletableCells(g.getSelectionCells())
            : g.getSelectionCells();
        if (cells === null || cells.length === 0) {
            return;
        }
        suppress += 1;
        try {
            mxClipboard.copy(g, cells);
            if (cut) {
                mxClipboard.removeCells(g, cells, true);
            }
        } finally {
            suppress -= 1;
        }
        clipboardOut(cut);
        if (cut) {
            commitNow(false);
        }
    }

    function deleteCells() {
        var g = graph();
        if (g === null) {
            return;
        }
        var cells = g.getDeletableCells(g.getSelectionCells());
        if (cells !== null && cells.length > 0) {
            g.removeCells(cells);
        }
    }

    // XML the system clipboard holds becomes diagram cells through the real
    // codec; anything else becomes a labelled vertex, the way draw.io's own
    // paste handles plain text.
    function pasteText(text) {
        var g = graph();
        if (g === null || typeof text !== "string" || text.length === 0) {
            return;
        }
        var node = null;
        var trimmed = text.replace(/^\uFEFF/, "");
        try {
            var parsed = mxUtils.parseXml(trimmed);
            var root = parsed !== null ? parsed.documentElement : null;
            if (root !== null) {
                if (root.nodeName === "mxGraphModel") {
                    node = root;
                } else if (root.nodeName === "mxfile") {
                    node = root.getElementsByTagName("mxGraphModel")[0] || null;
                } else if (root.nodeName === "root") {
                    var wrapper = mxUtils.createXmlDocument().createElement("mxGraphModel");
                    wrapper.appendChild(parsed.documentElement.cloneNode(true));
                    node = wrapper;
                }
            }
        } catch (error) {
            node = null;
        }
        if (node !== null) {
            try {
                var model = new mxGraphModel();
                new mxCodec(node.ownerDocument).decode(node, model);
                var cells = model.getChildCells(model.getRoot(), true, true);
                if (cells !== null && cells.length > 0) {
                    var parent = g.getDefaultParent();
                    var point = g.getInsertPoint ? g.getInsertPoint() : null;
                    g.importCells(cells, point !== null ? point.x : 30,
                        point !== null ? point.y : 30, parent);
                    g.setSelectionCells(cells);
                    return;
                }
            } catch (error) {
                node = null;
            }
        }
        var vertex = g.insertVertex(g.getDefaultParent(), null, text, 0, 0, 80, 40);
        try {
            g.updateCellSize(vertex);
        } catch (error) {
            // The label still stands; sizing is cosmetic.
        }
    }

    function installEditing() {
        var g = graph();
        if (g === null || viewer.editor === null) {
            return;
        }
        viewer.editor.editable = true;
        g.lightbox = false;
        g.setEnabled(true);
        g.setCellsEditable(true);
        g.setCellsMovable(true);
        g.setCellsResizable(true);
        g.setCellsSelectable(true);
        g.setConnectable(true);
        g.setDropEnabled(true);
        g.setPanning(true);
        if (g.panningHandler !== null && g.panningHandler !== undefined) {
            g.panningHandler.useLeftButtonForPanning = false;
        }
        var manager = viewer.editor.undoManager;
        if (manager === null || manager === undefined) {
            return;
        }
        manager.addListener(mxEvent.ADD, commitLater);
        manager.addListener(mxEvent.UNDO, commitLater);
        manager.addListener(mxEvent.REDO, commitLater);
    }

    function keyDown(event) {
        if (viewer === null || !loaded) {
            return;
        }
        var g = graph();
        if (g !== null && g.cellEditor !== null &&
            typeof g.cellEditor.isContentEditing === "function" &&
            g.cellEditor.isContentEditing()) {
            // Label editing — including IME composition — is entirely the
            // engine's; the workbench sees the result in the next commit.
            return;
        }
        var key = typeof event.key === "string" ? event.key.toLowerCase() : "";
        var ctrl = event.ctrlKey || event.metaKey;
        if (!ctrl && !event.altKey && !event.shiftKey &&
            (event.key === "Delete" || event.key === "Backspace")) {
            deleteCells();
            event.preventDefault();
            event.stopPropagation();
            return;
        }
        if (!ctrl || event.altKey) {
            return;
        }
        if (key === "z") {
            report(event.shiftKey ? "redo" : "undo", {});
        } else if (key === "y") {
            report("redo", {});
        } else if (key === "s") {
            report("save", {});
        } else if (key === "c") {
            copyCells(false);
            return;
        } else if (key === "x") {
            copyCells(true);
            return;
        } else if (key === "v") {
            report("paste", {});
        } else {
            return;
        }
        event.preventDefault();
        event.stopPropagation();
    }

    function applyCommand(kind, text) {
        if (viewer === null || !loaded) {
            return;
        }
        var g = graph();
        switch (kind) {
            case "sync":
                // A save asks for the live model even when nothing new was
                // committed on this surface; the sync marker lets the
                // workbench accept the same sequence again.
                commitNow(true);
                break;
            case "paste":
                pasteText(text);
                break;
            case "copy":
                copyCells(false);
                break;
            case "cut":
                copyCells(true);
                break;
            case "delete":
                deleteCells();
                break;
        }
    }

    function resetViewer() {
        suppress += 1;
        try {
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
            fileRoot = null;
            xmlPrefix = "";
            editSeq = 0;
            commitPath = null;
            syncPath = null;
        } finally {
            suppress -= 1;
        }
    }

    function parseXml(xml) {
        var stripped = xml.replace(/^\uFEFF/, "");
        var parsed = new DOMParser().parseFromString(stripped, "application/xml");
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
        return { root: root, stripped: stripped };
    }

    function applyPage(page) {
        pendingPage = page;
        if (viewer === null || !loaded) {
            return;
        }
        var pages = pageCount();
        var target = Math.max(0, Math.min(pages - 1, page));
        var g = graph();
        suppress += 1;
        try {
            // The page being left keeps its model in the file DOM before the
            // engine swaps it; its own undo stack is per-page, so the switch
            // clears it and reports the stash through the next commit.
            if (g !== null && g.isEditing()) {
                g.stopEditing(false);
            }
            writePageBack();
            if (typeof viewer.selectPage === "function" && pages > 1) {
                viewer.selectPage(target);
            }
            if (viewer.editor !== null && viewer.editor.undoManager !== null) {
                viewer.editor.undoManager.clear();
            }
        } finally {
            suppress -= 1;
        }
        reportState();
    }

    function applyZoom(command) {
        var g = graph();
        if (g === null || !loaded) {
            return;
        }
        try {
            if (command === "fit" && g.initialViewState !== undefined) {
                var initial = g.initialViewState;
                g.view.scaleAndTranslate(initial.scale, initial.translate.x,
                    initial.translate.y);
            } else if (command === "zoomIn") {
                g.zoomIn();
            } else if (command === "zoomOut") {
                g.zoomOut();
            } else if (command === "actual") {
                g.zoomActual();
            }
        } catch (error) {
            fail(error === null || error === undefined ? "zoom failed" : error.message);
            return;
        }
        reportState();
    }

    function load(xml, page, theme, commit, sync) {
        if (typeof xml !== "string" || xml.length === 0 || xml.length > MAX_XML) {
            fail("diagram exceeds the preview budget");
            return;
        }
        resetViewer();
        pendingPage = Math.max(0, page | 0);
        commitPath = commit;
        syncPath = sync;
        document.documentElement.style.colorScheme = theme === "dark" ? "dark" : "light";
        var container = document.getElementById("graph");
        try {
            var parsed = parseXml(xml);
            fileRoot = parsed.root;
            xmlPrefix = parsed.stripped.slice(0, parsed.stripped.indexOf("<" + fileRoot.nodeName));
            if (typeof GraphViewer !== "function") {
                throw new Error("draw.io viewer is unavailable");
            }
            viewer = new GraphViewer(container, fileRoot, {
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
            if (viewer.graph === null || viewer.editor === null) {
                var failedViewer = viewer;
                viewer = null;
                if (failedViewer.graph !== null &&
                    typeof failedViewer.graph.destroy === "function") {
                    failedViewer.graph.destroy();
                }
                throw new Error("draw.io viewer did not render");
            }
            if (viewer.editor.undoManager !== null) {
                viewer.editor.undoManager.clear();
            }
            suppress += 1;
            try {
                installEditing();
            } finally {
                suppress -= 1;
            }
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
            var commit = typeof message.commit === "string" ? message.commit : null;
            var sync = typeof message.sync === "string" ? message.sync : null;
            var route = /^\/drawio\/session\/[0-9a-f]{32}\/(commit|sync)$/;
            if (commit === null || !route.test(commit) || commit.indexOf("sync") >= 0 ||
                sync === null || !route.test(sync) || sync.indexOf("commit") >= 0) {
                commit = null;
                sync = null;
            }
            load(message.xml, message.page, message.theme === "dark" ? "dark" : "light",
                commit, sync);
        } else if (message.kind === "page" && Number.isInteger(message.page) &&
            message.page >= 0) {
            applyPage(message.page);
        } else if (message.kind === "fit" || message.kind === "zoomIn" ||
            message.kind === "zoomOut" || message.kind === "actual") {
            applyZoom(message.kind);
        } else if (message.kind === "undo" || message.kind === "redo" ||
            message.kind === "sync" || message.kind === "copy" ||
            message.kind === "cut" || message.kind === "delete") {
            applyCommand(message.kind, null);
        } else if (message.kind === "paste" && typeof message.text === "string") {
            applyCommand("paste", message.text);
        }
    });

    addListener("keydown", keyDown, true);

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
