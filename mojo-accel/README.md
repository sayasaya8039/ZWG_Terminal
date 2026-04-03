# Mojo SIMD 高速化モジュール

ZWG Terminal の推論パイプライン用 Mojo SIMD 最適化実装。

## アーキテクチャ

```
┌─────────────────────────────────────────────┐
│              ZWG Terminal (Rust)             │
│                                             │
│  ┌─────────────────┐  ┌──────────────────┐  │
│  │ flash_attention  │  │  kv_quantizer    │  │
│  │   .rs (AVX2)     │  │   .rs (AVX2)     │  │
│  │  #[cfg(x86_64)]  │  │  #[cfg(x86_64)]  │  │
│  └────────┬─────────┘  └────────┬─────────┘  │
│           │   C FFI (将来)       │            │
│  ┌────────▼─────────────────────▼──────────┐ │
│  │         Mojo SIMD レイヤー               │ │
│  │  flash_attention_simd.mojo              │ │
│  │  kv_quantizer_simd.mojo                 │ │
│  │  (SIMD width=8, vectorize, parallelize) │ │
│  └─────────────────────────────────────────┘ │
└─────────────────────────────────────────────┘
```

## ファイル構成

| ファイル | 内容 |
|---------|------|
| `flash_attention_simd.mojo` | Flash Attention 2.0 SIMD 実装 (Q×K内積, exp, V蓄積) |
| `kv_quantizer_simd.mojo` | Q8_0/Q4_0/TurboQuant SIMD 量子化・逆量子化 |
| `README.md` | このファイル |

## ビルド方法

### 前提条件

- WSL2 + Mojo SDK (modular CLI)
- Python venv に Mojo がインストール済み

### ビルド

```bash
# WSL 経由でビルド
wsl -e bash -c "source ~/mojo-env/bin/activate && cd /mnt/d/NEXTCLOUD/Windows_app/ZWG_Terminal/mojo-accel && mojo build flash_attention_simd.mojo -o flash_attention_simd"
wsl -e bash -c "source ~/mojo-env/bin/activate && cd /mnt/d/NEXTCLOUD/Windows_app/ZWG_Terminal/mojo-accel && mojo build kv_quantizer_simd.mojo -o kv_quantizer_simd"
```

### 共有ライブラリとしてビルド（C FFI 用）

```bash
# Rust から FFI 呼び出しする場合
wsl -e bash -c "source ~/mojo-env/bin/activate && cd /mnt/d/NEXTCLOUD/Windows_app/ZWG_Terminal/mojo-accel && mojo build --shared flash_attention_simd.mojo -o libflash_attention_simd.so"
wsl -e bash -c "source ~/mojo-env/bin/activate && cd /mnt/d/NEXTCLOUD/Windows_app/ZWG_Terminal/mojo-accel && mojo build --shared kv_quantizer_simd.mojo -o libkv_quantizer_simd.so"
```

## ベンチマーク実行

```bash
# Flash Attention ベンチマーク
wsl -e bash -c "source ~/mojo-env/bin/activate && cd /mnt/d/NEXTCLOUD/Windows_app/ZWG_Terminal/mojo-accel && mojo run flash_attention_simd.mojo"

# KV Quantizer ベンチマーク
wsl -e bash -c "source ~/mojo-env/bin/activate && cd /mnt/d/NEXTCLOUD/Windows_app/ZWG_Terminal/mojo-accel && mojo run kv_quantizer_simd.mojo"
```

## SIMD 最適化の詳細

### Flash Attention

| 処理 | スカラー | SIMD (width=8) |
|------|---------|---------------|
| Q×K 内積 | `a*b` の逐次 sum | `SIMD load → mul → reduce_add` |
| exp 計算 | `exp(s - max)` 逐次 | `SIMD exp(diff)` 8要素並列 |
| V 蓄積 | `out += w * v` 逐次 | `SIMD FMA: out + w_vec * v_vec` |
| スケーリング | `*= scale` 逐次 | `SIMD mul` 8要素並列 |

### KV Quantizer

| 処理 | スカラー | SIMD (width=8) |
|------|---------|---------------|
| max_abs 検出 | `fold(max, abs)` | `SIMD abs → max → reduce_max` |
| Q8 量子化 | `round/clamp` 逐次 | `SIMD div → round → clamp` |
| Q4 パッキング | ペアごと分岐 | LUT 化 + ペア処理 |
| 逆量子化 | `i8 * scale` 逐次 | `SIMD mul(scale_vec)` |

## Rust 側 SIMD (AVX2)

Rust 側では `#[cfg(target_arch = "x86_64")]` で AVX2 intrinsics を使用:

- `_mm256_fmadd_ps` — FMA (Q×K内積, V蓄積)
- `_mm256_max_ps` — max_abs リダクション
- `_mm256_round_ps` — 量子化 round
- `_mm256_div_ps` / `_mm256_mul_ps` — スケール演算

フォールバック: `#[cfg(not(target_arch = "x86_64"))]` で元のスカラー実装を維持。
