<p align="center">
  <img src="resources/icons/zwg_128.png" alt="ZWG Terminal" width="128" height="128">
</p>

<h1 align="center">ZWG Terminal</h1>

<p align="center">
  <strong>Ghostty VT + GPUI + ConPTY による高速 Windows ターミナルエミュレータ</strong>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/version-1.0.1-blue" alt="Version">
  <img src="https://img.shields.io/badge/platform-Windows%2011-0078D6" alt="Platform">
  <img src="https://img.shields.io/badge/license-MIT-green" alt="License">
  <img src="https://img.shields.io/badge/rust-2024%20edition-orange" alt="Rust">
  <img src="https://img.shields.io/badge/binary-8.6MB-brightgreen" alt="Size">
</p>

---

## 概要

ZWG Terminal は **Ghostty の VT パーサ**（Zig 製）と **Zed エディタの GPUI フレームワーク**（Rust 製）を組み合わせた、Windows ネイティブのターミナルエミュレータです。ConPTY を通じて PowerShell / CMD / WSL / Git Bash を実行し、GPU アクセラレーション（DX12）による高速レンダリングを実現します。

### アーキテクチャ

```
┌─────────────────────────────────────────────────┐
│                   ZWG Terminal                   │
├───────────┬───────────────┬─────────────────────┤
│  zwg-app  │  ghostty-vt   │  ghostty-vt-sys     │
│  Rust     │  Safe Wrapper │  Zig → C ABI → Rust │
│  GPUI UI  │  (Rust)       │  FFI Bindings       │
├───────────┴───────────────┴─────────────────────┤
│  GPUI 0.2 (Zed Editor Framework)                │
├─────────────────────────────────────────────────┤
│  ConPTY (Windows Pseudo Console)                │
├─────────────────────────────────────────────────┤
│  DX12 / Direct2D / DirectWrite                  │
└─────────────────────────────────────────────────┘
```

---

## 主な機能

| 機能 | 説明 |
|------|------|
| **マルチタブ** | タブの追加・切替・クローズ |
| **ペイン分割** | 水平・垂直分割、ドラッグリサイズ |
| **スニペットパレット** | テンプレート管理・検索・CSV インポート/エクスポート（Shift-JIS 対応） |
| **テーマ** | Catppuccin Mocha / Latte、Tokyo Night、Solarized、Monokai、Dracula、Nord |
| **設定パネル** | 7 カテゴリの GUI 設定（フォント、テーマ、キーボード 等） |
| **シェル自動検出** | pwsh / PowerShell / CMD / WSL / Git Bash |
| **ウィンドウ状態保存** | 位置・サイズを自動保存・復元 |
| **カスタムタイトルバー** | ネイティブ Win32 ドラッグ、トラフィックライトボタン |
| **GPU レンダリング** | Ghostty DX12 バックエンド（フォールバック: GPUI テキストシェイピング） |
| **非同期 I/O** | PTY 読み取り → VT パース を専用 Zig スレッドで実行 |

---

## キーバインド

| キー | アクション |
|------|-----------|
| `Ctrl+Shift+T` | 新規タブ |
| `Ctrl+Shift+W` | タブを閉じる |
| `Ctrl+Shift+D` | 右に分割（水平） |
| `Ctrl+Shift+E` | 下に分割（垂直） |
| `Ctrl+Shift+X` | アクティブペインを閉じる |
| `Ctrl+Tab` | 次のペインにフォーカス |
| `Ctrl+Shift+Tab` | 前のペインにフォーカス |
| `Ctrl+Shift+V` | スニペットパレットの表示/非表示 |
| `Ctrl+Shift+F` | スニペットキュー貼り付け |
| `Ctrl+Shift+Q` | 終了 |

---

## ビルド

### 必要なツール

| ツール | バージョン | 用途 |
|--------|-----------|------|
| **Rust** | 1.85+（2024 edition） | アプリケーション本体 |
| **Zig** | 0.15.2+ | Ghostty VT ライブラリのビルド |
| **Visual Studio Build Tools** | 2022+ | MSVC リンカ（`link.exe`） |
| **Git** | 最新 | サブモジュール管理 |

### 手順

```bash
# 1. リポジトリのクローン（サブモジュール含む）
git clone --recursive https://github.com/sayasaya8039/ZWG_Terminal.git
cd ZWG_Terminal

# 2. Ghostty サブモジュールの確認
git submodule update --init --recursive

# 3. リリースビルド
cargo build --release

# 4. 実行
./target/release/zwg.exe
```

### ビルド設定

リリースビルドは以下の最適化が適用されます:

```toml
[profile.release]
opt-level = 3          # 最大最適化
lto = "fat"            # リンク時最適化（全クレート）
codegen-units = 1      # 単一コード生成ユニット
strip = "symbols"      # シンボル除去
panic = "abort"        # パニック時即座に終了
```

---

## プロジェクト構成

```
ZWG_Terminal/
├── Cargo.toml                    # ワークスペース定義
├── vendor/ghostty/               # Ghostty サブモジュール (v1.3.0)
├── resources/
│   ├── icons/                    # アプリアイコン (16px ~ 256px + ICO)
│   └── ui/                       # SVG アイコン (settings, plus, copy 等)
└── crates/
    ├── zwg-app/                  # メインアプリケーション
    │   └── src/
    │       ├── main.rs           # エントリポイント、キーバインド登録
    │       ├── app.rs            # RootView、タブ管理、設定 UI、スニペット
    │       ├── config/mod.rs     # 設定、テーマ、永続化
    │       ├── shell/mod.rs      # シェル検出 (pwsh/cmd/wsl/git-bash)
    │       ├── split/mod.rs      # ツリー型ペイン分割
    │       ├── snippets/         # スニペットパレット (store/view/settings)
    │       └── terminal/
    │           ├── view.rs       # ターミナルペイン描画
    │           ├── pty.rs        # ConPTY プロセス管理
    │           ├── surface.rs    # Ghostty/fallback バックエンド切替
    │           ├── grid_renderer.rs  # グリフキャッシュ、グリッドレンダリング
    │           ├── gpu_view.rs   # DX12 GPU レンダリング
    │           └── vt_parser.rs  # フォールバック VT パーサ
    ├── ghostty-vt/               # Safe Rust ラッパー
    │   └── src/lib.rs
    └── ghostty-vt-sys/           # Zig FFI バインディング
        ├── build.rs              # Zig ビルドスクリプト
        ├── src/lib.rs            # C ABI 宣言
        └── zig/
            ├── lib.zig           # Ghostty VT ラッパー
            ├── build.zig         # Zig ビルド定義
            ├── content_scan.zig  # SIMD コンテンツ検出
            └── dx12.zig          # DX12 GPU レンダラー
```

---

## 設定

設定ファイルは `%APPDATA%/zwg/config.json` に自動保存されます。

### デフォルト設定

| 項目 | デフォルト値 |
|------|-------------|
| フォント | Cascadia Code, 14px |
| 行高 | 1.3 |
| テーマ | Catppuccin Mocha |
| スクロールバック | 10,000 行 |
| カーソル点滅 | 有効 |
| デフォルトサイズ | 120 列 × 30 行 |

### 対応シェル

| シェル | 検出方法 |
|--------|---------|
| PowerShell 7 (pwsh) | PATH 検索（優先） |
| PowerShell 5.1 | `powershell.exe`（フォールバック） |
| Command Prompt | `cmd.exe` |
| WSL | `wsl.exe` |
| Git Bash | `C:\Program Files\Git\bin\bash.exe` |

---

## 技術的な特徴

### Ghostty VT 統合

Ghostty (v1.3.0) の VT ターミナルパーサを Zig 経由の C ABI でリンクし、正確な VT100/xterm エスケープシーケンス処理を実現しています。`ghostty_vt` feature を無効にすると、内蔵の軽量 VT パーサにフォールバックします。

### GPUI フレームワーク

Zed エディタで使用されている GPUI 0.2 を採用。Direct2D/DirectWrite ベースの高品質テキストレンダリングと、宣言的 UI 構築を提供します。

### Win32 ネイティブ統合

- **カスタムタイトルバー**: `appears_transparent` + `DefWindowProcW(WM_NCLBUTTONDOWN)` による直接ドラッグ
- **ワークエリア制約**: `MonitorFromWindow` + `GetMonitorInfoW` でタスクバーを考慮した配置
- **ConPTY**: Windows Pseudo Console API によるネイティブなシェル実行

### メモリアロケータ

`mimalloc` を使用し、高頻度の小規模アロケーション（ターミナル出力バッファ等）を高速化しています。

---

## ライセンス

MIT License

Copyright (c) 2026 ZWG Terminal contributors
