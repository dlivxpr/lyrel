/*
 * Runs before the vendored draw.io viewer bundle.
 *
 * The viewer's own defaults point at viewer.diagrams.net. Nothing in this
 * process is allowed to leave the frozen https://drawio.lyrel.invalid origin,
 * so every resource base is redirected under that origin where the native
 * request handler answers 403. The response CSP is the second, independent
 * gate; neither one relies on the document or the engine behaving well.
 */
(function () {
    "use strict";

    var origin = window.location.origin;

    // bootstrap.js normally creates this global; the viewer bundle reads it.
    window.urlParams = {
        dev: "0",
        test: "0",
        pwa: "0",
        offline: "1",
        lockdown: "1",
        local: "1",
        plugins: "0",
        math: "0",
        nav: "0",
        lightbox: "0",
        embed: "1",
        chrome: "0",
        page: "0"
    };

    // All of these are same-origin dead ends. A missing path is a truthful
    // 403 from the native handler, never a public network request.
    window.PROXY_URL = origin + "/drawio/blocked";
    window.STYLE_PATH = origin + "/drawio/styles";
    window.SHAPES_PATH = origin + "/drawio/shapes";
    window.STENCIL_PATH = origin + "/drawio/stencils";
    window.GRAPH_IMAGE_PATH = origin + "/drawio/img";
    window.DRAW_MATH_URL = origin + "/drawio/math";
    window.DRAWIO_BASE_URL = origin + "/drawio";
    window.DRAWIO_VIEWER_URL = null;
    window.DRAWIO_LIGHTBOX_URL = null;
    window.DRAWIO_SERVER_URL = origin + "/drawio/blocked";
    window.EXPORT_URL = null;
    window.ICONSEARCH_PATH = null;
    window.ICON_SERVICE_PATH = null;
    window.mxBasePath = origin + "/drawio/mxgraph/";
    window.mxImageBasePath = origin + "/drawio/mxgraph/images/";
    window.mxLoadStylesheets = false;

    window.open = function () {
        return null;
    };
    window.alert = function () {};
    window.confirm = function () {
        return false;
    };
    window.prompt = function () {
        return null;
    };

    window.__drawioBooted = false;
    window.__drawioReady = null;

    window.__drawioBoot = function () {
        window.__drawioBooted = true;
        if (typeof window.__drawioReady === "function") {
            window.__drawioReady();
        }
    };

    window.onDrawioViewerLoad = function () {
        window.__drawioBoot();
    };
})();
