# HANDOFF.md — Vulkan P1-P2 + パフォーマンス最適化 引き継ぎ (2026-04-05)

## 今セッションで完了したこと

### 1. SIMD 4ホットパス ベクトル化 (`288e692`)

新規 `simd_ops.rs` (490行, 18テスト) に AVX2 ベクトル化を集約:

| ホットパス | 関数 | 効果 |
|-----------|------|------|
| VT パーサ | `scan_printable_ascii` / `find_escape` | 32B/cycle ASCII ラン検出 |
| dirty cell | `dirty_cells_from_bitmap` | sort+dedup → bitmap (O(n)) |
| UTF-8→glyph | `fast_cell_to_char_index` | ASCII テキスト O(1) |
| 色パッキング | `batch_or_alpha` + `init_default_row` | 8×u32/cycle |

### 2. 4ms バッチ非同期イベントループ (`3993199`)

settle loop (retry×4 + sweep×6 = 10+ lock/frame) を 3-phase パイプラインに置換:
- Phase 1: 4ms/2ms batch window
- Phase 2: Async parser settle (3×1ms)
- Phase 3: **1回** の backend.lock() → snapshot → cx.notify()

### 3. Vulkan GPU レンダラ Phase 0-2 (`9177154`, `f7fe302`)

Zig で Vulkan バックエンドを実装。DX12 と同一 FFI インターフェース:

```
crates/ghostty-vt-sys/zig/
  ├── vk.zig                    — 最小 Vulkan 型 + 動的関数ロード (~700行)
  ├── vulkan_renderer.zig       — VulkanRenderer + FFI exports (~660行)
  ├── shaders/terminal.{vert,frag}     — GLSL ソース
  └── shaders/terminal.{vert,frag}.spv — コンパイル済み SPIR-V

crates/ghostty-vt-sys/src/lib.rs  — extern "C" 宣言
crates/ghostty-vt/src/lib.rs      — VulkanRenderer Rust wrapper
```

**動作するもの**: VkInstance→VkDevice→VkQueue、storage buffer、readback buffer、VkRenderPass、VkPipeline (instanced draw)、command buffer recording、fence sync

---

## 残作業: P1 (Glyph Atlas + Swapchain) + P2 (Rust 統合)

### P1-A: DirectWrite glyph rasterization → VkImage

**目的**: DX12 renderer の `initGdi()` + `rasterizeGlyph()` を Vulkan 側に移植

**手順**:
1. `vulkan_renderer.zig` に DirectWrite/GDI フィールドを追加:
   ```zig
   gdi_dc: dx.HDC,
   gdi_font: dx.HFONT,
   dwrite_factory: ?*dx.IDWriteFactory,
   dwrite_gdi_interop: ?*dx.IDWriteGdiInterop,
   dwrite_font_face: ?*dx.IDWriteFontFace,
   glyph_baseline: f32,
   ```
2. `gpu_renderer.zig` の `initGdi()` / `rasterizeGlyph()` / `deinitGdi()` をコピー
   - DirectWrite/GDI は GPU API 非依存 → そのまま動く
   - `@import("dx12.zig")` で GDI/DirectWrite 型を取得 (既に使用中)
3. Atlas 用 VkImage (R8_UNORM, 2048×2048) + staging buffer を作成
4. `renderFrame()` 内で:
   - 未登録 codepoint → `rasterizeGlyph()` → `atlas_dirty = true`
   - `atlas_dirty` → staging buffer に atlas_bitmap コピー → `cmdCopyBufferToImage`
5. `init()` の `_ = font_size;` を `initGdi(font_size)` に変更

**参考**: `gpu_renderer.zig` の `initGdi()` (行233付近) と `rasterizeGlyph()` を参照

### P1-B: Fragment shader に atlas sampling 追加

**現在の terminal.frag**:
```glsl
out_color = in_bg;  // bg-only
```

**変更後**:
```glsl
layout(set = 0, binding = 1) uniform sampler2D atlas;
// ...
if (in_has_glyph > 0.5) {
    float alpha = texture(atlas, in_uv).r;
    out_color = mix(in_bg, vec4(in_fg.rgb, 1.0), alpha);
} else {
    out_color = in_bg;
}
```

**SPIR-V コンパイル問題**:
- `naga` は `uniform sampler2D` を GLSL モードでパース不可
- `glslangValidator` は scoop 版が MSVCR120.dll 不足で動作不可
- **解決策**: `scoop install vulkan-tools` または Vulkan SDK をインストールして `glslc` を使用
- WGSL に書き直して naga でコンパイルも可

**descriptor に atlas binding 追加**:
- `VkDescriptorSetLayoutBinding` に binding=1 (COMBINED_IMAGE_SAMPLER) 追加
- `VkSampler` 作成 (NEAREST filter, CLAMP_TO_EDGE)
- `VkDescriptorPoolSize` に IMAGE_SAMPLER 追加
- `VkWriteDescriptorSet` で atlas image info を書き込み

### P1-C: VkSwapchainKHR (native window presentation)

**vk.zig に追加する型**:
```zig
VkWin32SurfaceCreateInfoKHR
VkSwapchainCreateInfoKHR
VkSurfaceCapabilitiesKHR
VkSurfaceFormatKHR
VkPresentInfoKHR
VkSemaphoreCreateInfo
```

**VkFuncs に追加する関数**:
```
createWin32SurfaceKHR, destroySurfaceKHR
getPhysicalDeviceSurfaceSupportKHR
getPhysicalDeviceSurfaceCapabilitiesKHR
getPhysicalDeviceSurfaceFormatsKHR
createSwapchainKHR, destroySwapchainKHR
getSwapchainImagesKHR, acquireNextImageKHR
queuePresentKHR
createSemaphore, destroySemaphore
```

**VkInstance 作成時に拡張を有効化**:
```zig
const extensions = [_][*:0]const u8{ "VK_KHR_surface", "VK_KHR_win32_surface" };
inst_ci.enabledExtensionCount = 2;
inst_ci.ppEnabledExtensionNames = &extensions;
```

**VkDevice 作成時にデバイス拡張を有効化**:
```zig
const dev_extensions = [_][*:0]const u8{ "VK_KHR_swapchain" };
dev_ci.enabledExtensionCount = 1;
dev_ci.ppEnabledExtensionNames = &dev_extensions;
```

**新規 FFI**:
```zig
export fn ghostty_vulkan_renderer_init_swapchain(r, hwnd, w, h) u8;
export fn ghostty_vulkan_renderer_present(r) u8;
export fn ghostty_vulkan_renderer_resize_swapchain(r, w, h) u8;
```

### P2: gpu_view.rs ランタイムバックエンド選択

**`crates/zwg-app/src/terminal/gpu_view.rs` の変更**:

```rust
// 新しい enum
enum GpuBackend {
    Vulkan(ghostty_vt::VulkanRenderer),
    Dx12(ghostty_vt::GpuRenderer),
}

// GpuTerminalState の renderer フィールドを置換
pub(super) struct GpuTerminalState {
    backend: GpuBackend,
    // packed_rows, frame_cells 等は共通のまま
}

impl GpuTerminalState {
    pub fn new(width: u32, height: u32, font_size: f32) -> Option<Self> {
        // Vulkan first
        if let Ok(vk) = ghostty_vt::VulkanRenderer::new(width, height, font_size) {
            log::info!("Vulkan GPU renderer active");
            return Some(Self { backend: GpuBackend::Vulkan(vk), ... });
        }
        // DX12 fallback
        if let Ok(dx) = ghostty_vt::GpuRenderer::new(width, height, font_size) {
            log::info!("DX12 GPU renderer active (Vulkan unavailable)");
            return Some(Self { backend: GpuBackend::Dx12(dx), ... });
        }
        None  // → GPUI text shaping
    }
}
```

**render/resize は match で分岐**

---

## 推奨実装順序

```
1. P1-A: glyph rasterization (Zig)
   → gpu_renderer.zig の initGdi/rasterizeGlyph を vulkan_renderer.zig に移植
   → atlas VkImage + staging buffer

2. P1-B: fragment shader 更新
   → glslc インストール (scoop install vulkan-tools)
   → terminal.frag に atlas sampling
   → SPIR-V 再コンパイル

3. P2: gpu_view.rs (readback モード)
   → GpuBackend enum
   → Vulkan → DX12 → GPUI フォールバック
   → readback 経由で動作確認

4. P1-C: VkSwapchainKHR (native presentation)
   → vk.zig + vulkan_renderer.zig に swapchain
   → gpu_view.rs の present_native を Vulkan 対応
```

## ビルド

```bash
taskkill.exe /F /IM zwg.exe 2>/dev/null || true
cargo zigbuild --release -p zwg

# テスト
cargo test -p zwg -- terminal::simd_ops  # 18テスト
cargo test -p zwg -- terminal::           # 100+ テスト (既存3件は pre-existing failure)
```

## 重要な技術メモ

- `vk.zig` の `VkFuncs` は comptime reflection で自動ロード — フィールド名 = Vulkan 関数名 (vk プレフィックスなし)
- `GpuCellData` は DX12/Vulkan 共通 (20 bytes, `#[repr(C)]`)
- `atlas_bitmap` は R8 (1B/px), 2048×2048 = 4MB, CPU 上に常駐
- push constants = 48 bytes (viewport_size, cell_size, atlas_pitch/glyph_inv, term_cols, atlas_grid_cols)
- `vulkan-1.dll` 非存在 → VulkanRenderer::init() = null → DX12 フォールバック
