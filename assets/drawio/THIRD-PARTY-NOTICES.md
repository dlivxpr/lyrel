# draw.io viewer engine — third-party notices

Lyrel distributes one fixed draw.io viewer bundle for the workbench's local
diagram preview and editing:

- `viewer-static.min.js` — the official `jgraph/drawio` v32.6.10 release build
  named `js/viewer-static.min.js` in `draw.war`.

The full Apache-2.0 text for draw.io is in `LICENSE`. The component licenses
summarised below are distributed verbatim in `LICENSES/`. SHA-256 values for
every distributed file are in `PROVENANCE.json` and are checked by
`crate::ui::code_editor::drawio_engine::tests`.

## What the bundle contains

`viewer-static.min.js` is built from this fixed source composition (see
`etc/build/build.xml` at the pinned commit):

1. `base-viewer.min.js`: `mxClient`/`client.min.js`,
   draw.io `Editor`/`EditorUi`/`Graph`/`Shapes`/`Actions`,
   diagramly `DrawioFile`/`LocalFile`/`Editor`/`EditorUi`/`Pages`/`Trees`/
   `ForceExplorer`/`Minimal`/`DrawioComment`/`DrawioUser`, Graph-Resources,
   Graph-Stylesheet, and `GraphViewer`;
2. `shapes-14-6-5.min.js`;
3. `stencils.min.js`;
4. the viewer boot snippet (service-worker registration gated by `pwa`,
   `Editor.initMath()`, `GraphViewer.processElements()`).

It deliberately does **not** contain:

- `extensions.min.js` (JSZip, drawio-vsd, drawio-elk, drawio-mermaid,
  Bridge/OrgChart, libavoid). Those are not distributed: the fixed release has
  no public corresponding source for the LGPL-2.1 libavoid wasm2js build or
  the EPL-2.0 ELK port, so their obligations cannot be closed by this bundle;
- `math4/` (MathJax). Math is disabled with `math=0`; the engine never fetches
  it because the native response handler refuses the path and CSP blocks
  remote URLs;
- `templates/` (CC-BY-4.0), `img/`, `styles/fonts/ArchitectsDaughter-Regular.ttf`,
  PlantUML, Java server components, or Office/Visio converters.

## Vendored JavaScript licenses

| Component | Version / source | License | Distributed text |
| --- | --- | --- | --- |
| draw.io / mxGraph portions in draw.io | v32.6.10 | Apache-2.0 (draw.io root `LICENSE`); mxGraph v4.2.2 Apache-2.0 | `LICENSE`, `LICENSES/Apache-2.0-mxgraph-4.2.2.txt` |
| spin.js | 2.0.0 | MIT, Copyright (c) 2011 Felix Gnass | `LICENSES/MIT-spin-2.0.0.txt` |
| DOMPurify | 3.4.17 | MPL-2.0 OR Apache-2.0 — **Lyrel elects Apache-2.0** | `LICENSES/Apache-2.0-DOMPurify-3.4.17.txt` |
| regenerator-runtime (inside DOMPurify) | Babel helper | MIT, Copyright (c) 2014-present Facebook, Inc. and Babel contributors | see the DOMPurify header; MIT text in `LICENSES/MIT-spin-2.0.0.txt` applies to the MIT form |
| pako | 2.2.0 (includes zlib 1.3.2 JS port) | MIT AND Zlib | `LICENSES/MIT-pako-2.2.0.txt`, `LICENSES/Zlib-pako-2.2.0.txt` |
| roughjs | 4.6.6 | MIT, Copyright (c) 2019 Preet Shihn | `LICENSES/MIT-roughjs-4.6.6.txt` |
| hachure-fill (inside rough) | ^0.5.2 | MIT, Copyright (c) 2023 Preet Shihn | `LICENSES/MIT-hachure-fill.txt` |
| path-data-parser (inside rough) | ^0.1.0 | MIT, Copyright (c) 2020 Preet Shihn | `LICENSES/MIT-path-data-parser.txt` |
| points-on-curve (inside rough) | ^0.2.0 | MIT, Copyright (c) 2020 Preet Shihn | `LICENSES/MIT-points-on-curve.txt` |
| points-on-path (inside rough) | ^0.2.1 | MIT, Copyright (c) 2020 Preet | `LICENSES/MIT-points-on-path.txt` |

The second MIT text above is a duplicate of the spin.js MIT license text; the
notice is the important part for regenerator. No CryptoJS, JSZip, MathJax,
Mermaid, ELK, libavoid, PlantUML or perfect-freehand code is part of
`viewer-static.min.js`.

## Icon, shape and stencil terms

The bundle embeds the draw.io shape bundle and stencil bundle. Upstream does
not license those resources under the root Apache-2.0 alone:

- `shapes-LICENSE.txt` — the upstream `shapes/LICENSE` terms, distributed
  verbatim;
- `stencils-LICENSE.txt` — the upstream `stencils/LICENSE` terms, distributed
  verbatim.

Those terms restrict use of the icon/stencil libraries in Atlassian products,
marketplaces and plugins; they do not restrict end-user diagram exports and do
not transfer to diagrams authored by Lyrel users. Lyrel does not claim that
embedded third-party icon sets are Apache-2.0. Resources outside the two
bundles (`img/`, templates, fonts) are not distributed.

## Changes and security posture

Lyrel serves the engine bytes unmodified. It does not patch the vendor bundle.
The embedding layer is Lyrel-owned (`shell.html`, `shell.js`, `engine.html`,
`engine-boot.js`, `engine-bridge.js`) and:

- replaces every remote resource base with the reserved
  `https://drawio.lyrel.invalid` origin;
- sets `pwa=0`, so the viewer does not register a service worker;
- sets `math=0`, so MathJax is neither loaded nor distributed;
- disables stencil eval in the vendor boot (`allowEval=!1`) and blocks
  `unsafe-eval` with the response CSP;
- treats `extensions.min.js`, plugins, cloud services, remote export and
  on-demand external libraries as unavailable rather than silently dropping
  diagram content;
- renders and edits locally: the engine's current model returns only as a
  serialized mxfile through this session's own sequence-gated commit/sync
  POST, and the workbench writes the file only for Save or Save As through the
  owning Host. No merge, patch, upload, remote-export, cloud or arbitrary-path
  request is forwarded by the native bridge.

DOMPurify 3.4.17 is the fixed sanitizer contained in this release; the older
v30 audit findings do not apply to this bundle. The preview also runs inside a
same-origin sandboxed iframe and a CSP that forbids inline scripts and eval.
