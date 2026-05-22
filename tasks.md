# WASM Frame Buffer — Tasks

## Problem

Every viewer we add to Zed (pdf_viewer, image_viewer, typst_viewer) is a new Rust crate compiled into the Zed binary. Each brings its own dependencies (poppler, usvg, resvg, etc.), its own GPUI view boilerplate, and its own WebSocket/LSP plumbing. When the typst extension already launches tinymist as a language server, there's no reason the rendering pipeline should live inside Zed's binary — it should live in the extension.

The missing piece: extensions have no way to push pixels to the screen. The WASM extension API can spawn processes, talk HTTP, run LSP — but it can't display an image. If it could write BGRA pixel data into a frame buffer that GPUI displays, all the viewer logic (SVG rasterisation, PDF rendering, video decoding) moves into extensions.

## Goal

Add a frame buffer mechanism to the Zed extension WASM host that lets an extension:
1. Create a named frame buffer with dimensions (width × height)
2. Write BGRA pixel data into it (full frame or partial update)
3. Have GPUI display it in a pane (scrollable, zoomable, multi-page)

Stress-test the design with 4K video data (~33ms per frame, ~32MB/s of pixel throughput) to prove the WASM boundary isn't a bottleneck. Then use the same mechanism for typst preview, PDF viewing, image viewing — all as extensions rather than compiled-in crates.

## Architecture sketch

```
Extension (WASM)                          Zed host
─────────────────                         ────────────────
tinymist LSP ──→ SVG bytes                frame_buffer_create(id, w, h)
usvg + resvg (compiled to WASM)    ──→    frame_buffer_write(id, offset, &[u8])
  or: receive pre-rasterised pixels       frame_buffer_present(id)
                                               │
                                          GPUI: RenderImage from shared buffer
                                          img() element in viewer pane
```

## Key questions

### WASM data transfer
- wasmtime shared memory: can the host map the WASM linear memory directly to avoid copying pixel data across the boundary?
- Alternative: host provides a buffer (resource handle), extension writes into it via `frame_buffer_write(handle, offset, data)`, host reads it back without copying through WASM return values
- At 4K (3840×2160×4 = 33MB per frame, 30fps = ~1GB/s), memcpy cost matters. Need zero-copy or single-copy.

### WIT interface
- New WIT resource type: `frame-buffer`
- Methods: `create(width: u32, height: u32) -> frame-buffer`, `write(buf: frame-buffer, offset: u32, data: list<u8>)`, `present(buf: frame-buffer)`, `destroy(buf: frame-buffer)`
- Versioning: this would be a new WIT since_v0.5.0 addition
- Multi-page: `create-paged(page-count: u32, width: u32, height: u32)` or just multiple buffers?

### GPUI integration
- The host side of `present()` wraps the buffer as `Arc<RenderImage>` and notifies the view
- The viewer pane is generic — it doesn't know about typst, PDF, or video. It just displays frame buffers from the extension.
- Scroll, zoom, page navigation all live in the host-side viewer pane (reuse pdf_viewer patterns)
- Scale factor handling: extension writes at device pixels, host knows the scale factor

### What moves into extensions
- **typst_viewer**: the extension already launches tinymist. Add usvg+resvg to the WASM build (or have tinymist send pre-rasterised pixels). Write frames to the buffer.
- **pdf_viewer**: poppler (or a Rust PDF renderer) compiled to WASM. Extension owns the rendering.
- **image_viewer**: trivial — decode image, write pixels to buffer.
- **video**: ffmpeg/libav bindings in WASM, decode frames, write to buffer. This is the stress test.

## Phases

### Phase 1: WIT + host plumbing
- Define the `frame-buffer` WIT resource in since_v0.5.0
- Implement host-side: buffer allocation, `write()` that copies from WASM memory, `present()` that wraps as RenderImage
- Generic viewer pane that displays frame buffers (steal layout from pdf_viewer)
- Test with a trivial extension that writes a solid colour

### Phase 2: 4K video stress test
- Extension that decodes video (embedded test clip or from a URL)
- 30fps frame writes at 3840×2160
- Measure: WASM→host transfer time, GPUI display latency, total frame budget
- Identify bottlenecks: is it the copy? The GPUI texture upload? The WASM call overhead?
- Optimise: shared memory, partial updates (dirty rects), double buffering

### Phase 3: Typst preview as extension
- Move typst_viewer logic into the typst extension
- Extension talks to tinymist (it already does), receives SVG, rasterises with resvg (compiled to WASM), writes pixels to frame buffer
- Delete the typst_viewer crate from Zed
- Prove the latency is acceptable (target: <150ms end-to-end at 80 wpm)

### Phase 4: PDF + image viewers as extensions
- Port pdf_viewer rendering to a PDF extension
- Port image_viewer to use frame buffers
- These are lower priority — the pattern is proven by typst

## Benchmarks needed

| Scenario | Target | Metric |
|----------|--------|--------|
| Solid colour 1080p | <1ms | write + present round-trip |
| 4K frame write (33MB) | <10ms | WASM→host memcpy |
| 4K 30fps sustained | 33ms budget | end-to-end per frame |
| Typst preview (A4 2-col) | <150ms | edit → pixels displayed |
| Typst preview (simple) | <80ms | edit → pixels displayed |

## References

- `crates/extension_host/src/wasm_host/wit.rs` — current WIT host bindings
- `crates/extension_api/wit/since_v0.4.0/` — current WIT definitions
- `crates/gpui/src/assets.rs` — `RenderImage`, `ImageId`, frame buffer concepts
- `crates/pdf_viewer/src/pdf_viewer.rs` — scroll, zoom, multi-page layout patterns
- `crates/typst_viewer/` — what we want to eventually move into an extension
