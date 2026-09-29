# Website

The GitHub Pages presentation, sandbox and guide are plain HTML, CSS and ES modules.
The sandbox calls the real Rust library in a Web Worker through a separate WASM bridge.
Neither the published crate nor the bridge has third-party Rust dependencies.

Build and verify:

```sh
rustup target add wasm32-unknown-unknown
python3 scripts/build-site.py
node --test site/tests/engine.test.mjs
python3 -m http.server 8080 --directory _site
```

Open http://localhost:8080/. The build includes complete local rustdoc under `/api/`,
compiles every Rust guide example and checks local navigation and API links.
Do not open the HTML directly with `file://`: workers and WASM need an HTTP origin.

The Pages workflow builds on pull requests and deploys `main`. Enable GitHub Pages
with **GitHub Actions** as its source. No API keys, server or audio upload endpoint
are needed. Files stay in browser memory. The bridge enforces the sandbox's input limits.

Generated `_site/`, the bridge's `target/`, and review captures are not committed.
The WASM binary is produced from source at deployment time, rather than stored in Git.
