# filebrowser-lite-wasi

Minimal WASI build of File Browser with the original frontend embedded into a single `.wasm`.

Requires `wasmtime` with `wasmtime serve` support.

## Quick Start

Download the latest release asset:

```bash
curl -L https://github.com/enbop/filebrowser-lite/releases/latest/download/filebrowser-lite-wasi.wasm -o filebrowser-lite-wasi.wasm
```

Create a local data directory and serve the wasm:

```bash
mkdir -p data
wasmtime serve --addr=0.0.0.0:8082 -Scli --dir data ./filebrowser-lite-wasi.wasm
```

Open `http://localhost:8082/`.

Or open `http://<your-machine-ip>:8082/` from another machine on the same network.

## Scope

This build only keeps the core file APIs:

- directory listing
- file metadata
- raw file download
- directory creation
- raw body file upload
- overwrite existing file content
- rename or copy
- delete

It intentionally skips authentication, users, shares, previews, search, and TUS uploads.

## Build From Source

Build from the repository root in this order so the wasm embeds the latest original frontend dist:

```bash
cd frontend
corepack pnpm install --frozen-lockfile
FILEBROWSER_LITE_WASI=1 corepack pnpm exec vite build

cd ../filebrowser-lite-wasi
cargo build --target=wasm32-wasip2 -r
```

If you want to force a fully fresh wasm rebuild:

```bash
cd filebrowser-lite-wasi
cargo clean
cargo build --target=wasm32-wasip2 -r
```

Run the locally built wasm with:

```bash
mkdir -p data
wasmtime serve --addr=0.0.0.0:8082 -Scli --dir data ./target/wasm32-wasip2/release/filebrowser-lite-wasi.wasm
```

## Notes

- Uploads are supported in lite mode and currently send the file bytes as the raw HTTP request body.
- Multipart support is intentionally not included in this first validation step. `multipart/form-data` is the browser form upload format that wraps files and fields with MIME boundaries; the current backend does not parse that format.
- Request paths are normalized and reject `..` traversal.
- The guest only sees what was mounted through `wasmtime serve --dir`.
- The embedded frontend is built from the original `frontend/` app in lite mode and must be rebuilt before recompiling the wasm when frontend assets change.
- On the current `wstd` stack, `wasmtime serve` needs `-Scli` so the component can link the expected `wasi:cli/*` imports.