# filebrowser-lite-wasi

Minimal validation prototype for a WASI build of a filebrowser-like backend.

## Scope

This prototype only keeps the core file APIs:

- directory listing
- file metadata
- raw file download
- directory creation
- raw body file upload
- overwrite existing file content
- rename or copy
- delete

It intentionally skips authentication, users, shares, previews, search, TUS uploads, and advanced UI configuration.

## Build

Build from the repository root in this order so the wasm embeds the latest original frontend dist:

```bash
cd frontend
corepack pnpm install --frozen-lockfile
FILEBROWSER_LITE_WASI=1 corepack pnpm exec vite build

cd ../filebrowser-lite-wasi
cargo build --target=wasm32-wasip2 -r
```

If you previously built a wasm that still embedded the old handwritten prototype UI, force a fresh rebuild once:

```bash
cd filebrowser-lite-wasi
cargo clean
cargo build --target=wasm32-wasip2 -r
```

## Run

Create a host directory to expose to the guest:

```bash
mkdir -p data
wasmtime serve --addr=0.0.0.0:8082 -Scli --dir data ./target/wasm32-wasip2/release/filebrowser-lite-wasi.wasm
```

Open http://localhost:8082/ for the embedded filebrowser frontend.

## Example API calls

```bash
curl http://localhost:8082/api/health
curl http://localhost:8082/api/resources/
curl -X POST --data-binary @README.md http://localhost:8082/api/resources/demo/readme-copy.md
curl http://localhost:8082/api/resources/demo/readme-copy.md
curl http://localhost:8082/api/raw/demo/readme-copy.md -o /tmp/readme-copy.md
curl -X PATCH "http://localhost:8082/api/resources/demo/readme-copy.md?action=rename&destination=/demo/readme-renamed.md"
curl -X DELETE http://localhost:8082/api/resources/demo/readme-renamed.md
```

## Notes

- Uploads are supported in lite mode and currently send the file bytes as the raw HTTP request body.
- Multipart support is intentionally not included in this first validation step. `multipart/form-data` is the browser form upload format that wraps files and fields with MIME boundaries; the current backend does not parse that format.
- Request paths are normalized and reject `..` traversal.
- The guest only sees what was mounted through `wasmtime serve --dir`.
- The embedded frontend is built from the original `frontend/` app in lite mode and must be rebuilt before recompiling the wasm when frontend assets change.
- If the browser still shows the old handwritten prototype UI after rebuilding, do a hard refresh and verify the served HTML contains `<title>File Browser</title>` rather than the old prototype page.
- On the current `wstd` stack, `wasmtime serve` needs `-Scli` so the component can link the expected `wasi:cli/*` imports.