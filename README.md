<p align="center">
  <img src="resources/icons/zwg_128.png" alt="ZWG Terminal" width="128" height="128">
</p>

<h1 align="center">ZWG Terminal</h1>

<p align="center">
  <strong>Ghostty VT + GPUI + ConPTY による高速 Windows ターミナルエミュレータ</strong>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/version-1.3.0-blue" alt="Version">
  <img src="https://img.shields.io/badge/platform-Windows%2011-0078D6" alt="Platform">
  <img src="https://img.shields.io/badge/license-MIT-green" alt="License">
  <img src="https://img.shields.io/badge/rust-2024%20edition-orange" alt="Rust">
  <img src="https://img.shields.io/badge/zig-0.15-F7A41D" alt="Zig">
</p>

---

## スクリーンショット

![ZWG Terminal Settings](tmp_figma_settings.png)

---

## ダウンロード

> **ビルド不要でインストールできます**

| ファイル | 説明 |
|---------|------|
| **`ZWG_Terminal_Setup.exe`** | GUI インストーラー（ショートカット作成・レジストリ登録・アンインストーラー付き） |
| **`zwg.exe`** | ポータブル版（単体実行可能） |

[GitHub Releases](https://github.com/sayasaya8039/ZWG_Terminal/releases) からダウンロードしてください。

---

## 概要

ZWG Terminal は **Ghostty の VT パーサ**（Zig 製）と **Zed エディタの GPUI フレームワーク**（Rust 製）を組み合わせた、Windows ネイティブのターミナルエミュレータです。ConPTY を通じて PowerShell / CMD / WSL / Git Bash を実行し、GPU アクセラレーション（DX12）による高速レンダリングを実現します。

### アーキテクチャ

```
┌─────────────────────────────────────────────────────┐
│                    ZWG Terminal                      │
├───────────┬────────────────┬────────────────────────┤
│  zwg-app  │  ghostty-vt    │  ghostty-vt-sys        │
│  Rust     │  Safe Wrapper  │  Zig → C ABI → Rust    │
│  GPUI UI  │  (Rust)        │  FFI Bindings          │
├───────────┴────────────────┴────────────────────────┤
│  zwg-intelligence (AI / Rendering Acceleration)     │
├─────────────────────────────────────────────────────┤
│  GPUI 0.2 (Zed Editor Framework)                    │
├─────────────────────────────────────────────────────┤
│  ConPTY (Windows Pseudo Console)                    │
├─────────────────────────────────────────────────────┤
│  DX12 / Direct2D / DirectWrite                      │
└─────────────────────────────────────────────────────┘
```

---

## 主な機能

| 機能 | 説明 |
|------|------|
| **マルチタブ** | タブの追加・切替・クローズ |
| **ペイン分割** | 水平・垂直分割、ドラッグリサイズ |
| **右クリックコンテキストメニュー** | ペイン管理（分割・クローズ・コピー・ペースト） |
| **tmux 互換 IPC** | Claude Code エージェントチームの可視化・ペイン自動分割 |
| **GUI 設定パネル** | 7 カテゴリの設定 UI（一般・外観・ターミナル・キーボード・通知・プライバシー・詳細） |
| **マルチ AI サジェスト** | コマンドパレット入力中に Claude / OpenAI / Gemini API で候補コマンドを即時表示 |
| **スニペットパレット** | クリップボード履歴 + 定型文管理・検索・新規作成・編集・ターミナルへ直接ペースト（日本語 IME 対応） |
| **Intelligence モジュール** | Flash Attention、投機的デコーディング、KV 量子化、PagedAttention による Ollama 高速化 |
| **テーマ** | Catppuccin Mocha / Latte、Tokyo Night、Solarized、Monokai、Dracula、Nord |
| **背景画像** | カスタム背景画像（透過度調整可能） |
| **シェル自動検出** | pwsh / PowerShell / CMD / WSL / Git Bash |
| **ウィンドウ状態保存** | 位置・サイズを自動保存・復元 |
| **カスタムタイトルバー** | ネイティブ Win32 ドラッグ、トラフィックライトボタン |
| **GPU レンダリング** | DX12 ネイティブ GPU レンダラー + Ghostty VT バックエンド |
| **高速レンダリング** | Alacritty 方式 PTY 1MB バッファ、ASCII グリフプリロード、ダメージダブルバッファ |
| **非同期 I/O** | PTY 読み取り → VT パース を専用 Zig スレッドで実行 |
| **キーバインドカスタマイズ** | 設定 UI からショートカットキーを変更可能 |
| **通知設定** | ベル音・ビジュアルベル・プロセス完了通知の個別設定 |
| **インストーラー** | PyInstaller 製 GUI インストーラー（ショートカット作成・レジストリ登録・アンインストーラー付き） |

---

## v1.3.0 の変更点

### 新機能

- **tmux 互換 IPC レイヤー** — Claude Code の TeamCreate によるエージェントチームをペイン分割で可視化
- **右クリックコンテキストメニュー** — ペイン分割・クローズ・コピー・ペーストを右クリックから実行
- **Intelligence モジュール** (`zwg-intelligence` クレート) — Flash Attention、投機的デコーディング、KV 量子化、PagedAttention、NVFP4、スライディングウィンドウ、並列デコーディング
- **env-shim 自動生成** — PowerShell チームメイトスポーン用の環境変数注入

### パフォーマンス改善

- **Alacritty 方式高速化** — PTY 1MB バッファ、ASCII グリフプリロード、ダメージダブルバッファ
- **レンダリングパイプライン最適化** — GPU デルタレンダリング、適応フレームペーシング、SIMD 最適化
- **入力遅延削減** — クリップボード監視のオーバーヘッド削減、キーストローク時のデバッグ I/O 除去
- **fast_io / input_accel / multi_nic** — I/O アクセラレーション、入力予測、マルチ NIC 最適化

### バグ修正

- IPC メッセージフラグメンテーション修正（Windows Named Pipes）
- IME 二重入力・テキスト入力バグ 7 件修正（5 エージェント並行調査）
- AttachConsole がパイプ stdout を上書きする問題を修正
- PATH 処理を「削除→先頭再配置」に変更しチームスポーン障害を解消
- スニペットパレットのゴースト選択・重複 ID・スクロール問題を修正
- tmux ペイン検出失敗を修正

---

## キーバインド（デフォルト）

| キー | アクション |
|------|-----------|
| `Ctrl+Shift+T` | 新規タブ |
| `Ctrl+Shift+W` | タブを閉じる |
| `Ctrl+Shift+D` | 右に分割（水平） |
| `Ctrl+Shift+S` | 下に分割（垂直） |
| `Ctrl+Shift+X` | アクティブペインを閉じる |
| `Ctrl+Tab` | 次のペインにフォーカス |
| `Ctrl+Shift+Tab` | 前のペインにフォーカス |
| `Ctrl+Shift+V` | スニペットパレットの表示/非表示 |
| `Ctrl+Shift+F` | スニペットキュー貼り付け |
| `Ctrl+E` | スニペット編集 |
| `Ctrl+,` | 設定を開く |
| `Ctrl+Shift+Q` | 終了 |

> 設定パネルの「キーボード」タブからカスタマイズできます。

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
cargo zigbuild --release -p zwg-app

# 4. 実行
./target/release/zwg.exe
```

### ビルド設定

リリースビルドは以下の最適化が適用されます:

```toml
[profile.release]
opt-level = 3          # 最大最適化
lto = "thin"           # Thin LTO（速度と最適化のバランス）
codegen-units = 1      # 単一コード生成ユニット（最大最適化）
strip = true           # シンボル除去
panic = "abort"        # パニック時即座に終了
overflow-checks = false # オーバーフロー検査無効化
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
├── installer/
│   ├── zwg_installer.py          # GUI インストーラー (PyInstaller)
│   ├── zwg_uninstaller.py        # アンインストーラー
│   └── build_installer.py        # インストーラービルドスクリプト
└── crates/
    ├── zwg-app/                  # メインアプリケーション
    │   └── src/
    │       ├── main.rs           # エントリポイント、キーバインド登録
    │       ├── app.rs            # RootView、タブ管理、設定 UI、スニペット
    │       ├── ai.rs             # マルチ AI サジェスト (Claude/OpenAI/Gemini)
    │       ├── config/mod.rs     # 設定、テーマ、キーボード、永続化
    │       ├── shell/mod.rs      # シェル検出 (pwsh/cmd/wsl/git-bash)
    │       ├── split/mod.rs      # ツリー型ペイン分割
    │       ├── snippet_palette.rs # クリップボード履歴 + 定型文データモデル
    │       ├── template_editor.rs # 定型文エディタモーダル（IME 対応）
    │       ├── clipboard_monitor.rs # クリップボード監視
    │       └── terminal/
    │           ├── view.rs       # ターミナルペイン描画
    │           ├── pty.rs        # ConPTY プロセス管理
    │           ├── surface.rs    # Ghostty/fallback バックエンド切替
    │           ├── grid_renderer.rs  # グリフキャッシュ、グリッドレンダリング
    │           ├── gpu_view.rs   # DX12 GPU レンダリング
    │           ├── native_gpu_presenter.rs  # DX12 ネイティブプレゼンター
    │           └── vt_parser.rs  # フォールバック VT パーサ
    ├── zwg-intelligence/         # AI / レンダリング高速化モジュール
    │   └── src/
    │       ├── fast_io.rs        # 高速 I/O アクセラレーション
    │       ├── input_accel.rs    # 入力予測・高速化
    │       ├── flash_attention.rs # Flash Attention 実装
    │       ├── speculative_decoder.rs # 投機的デコーディング
    │       ├── kv_quantizer.rs   # KV キャッシュ量子化
    │       ├── paged_attention.rs # PagedAttention
    │       ├── render_accel.rs   # レンダリング高速化
    │       └── ollama_client.rs  # Ollama クライアント
    ├── ghostty-vt/               # Safe Rust ラッパー
    │   └── src/lib.rs
    └── ghostty-vt-sys/           # Zig FFI バインディング
        ├── build.rs              # Zig ビルドスクリプト
        ├── src/lib.rs            # C ABI 宣言
        └── zig/
            ├── lib.zig           # Ghostty VT ラッパー
            ├── content_scan.zig  # SIMD コンテンツ検出
            ├── gpu_renderer.zig  # GPU レンダリングパイプライン
            ├── shaders.zig       # HLSL シェーダー定義
            └── dx12.zig          # DX12 GPU レンダラー
```

---

## 設定

設定ファイルは `%APPDATA%/zwg/config.json` に自動保存されます。
設定パネルは `Ctrl+,` で開けます。

### 設定カテゴリ

| カテゴリ | 内容 |
|---------|------|
| **一般** | シェル選択、起動時の動作、ログイン時自動起動 |
| **外観** | テーマ、フォント、行高、背景画像 |
| **ターミナル** | スクロールバック行数、カーソル点滅、デフォルトサイズ |
| **キーボード** | ショートカットキーのカスタマイズ |
| **通知** | ベル音、ビジュアルベル、プロセス完了通知 |
| **プライバシー** | 選択時コピー、終了時履歴クリア |
| **詳細** | AI サジェスト設定、設定のリセット |

### マルチ AI サジェスト

コマンドパレット入力中の AI サジェストは環境変数または設定パネルで有効化します。いずれか 1 つ以上の API キーを設定すると利用可能です。

| プロバイダ | API キー環境変数 | モデル環境変数 | デフォルトモデル |
|-----------|-----------------|---------------|-----------------|
| **Claude** | `ANTHROPIC_API_KEY` | `ZWG_CLAUDE_MODEL` | `claude-3-5-haiku-latest` |
| **OpenAI** | `OPENAI_API_KEY` | `ZWG_OPENAI_MODEL` | `gpt-4.1-mini` |
| **Gemini** | `GEMINI_API_KEY` | `ZWG_GEMINI_MODEL` | `gemini-2.0-flash` |

```powershell
# 例: Claude を使う場合
$env:ANTHROPIC_API_KEY = "your_api_key"
```

- ベース URL のカスタマイズ: `ZWG_ANTHROPIC_BASE_URL` / `ZWG_OPENAI_BASE_URL` / `ZWG_GEMINI_BASE_URL`

### デフォルト設定

| 項目 | デフォルト値 |
|------|-------------|
| フォント | Consolas, 14px |
| 行高 | 1.3 |
| テーマ | Catppuccin Mocha |
| スクロールバック | 10,000 行 |
| カーソル点滅 | 有効 |
| デフォルトサイズ | 120 列 x 30 行 |

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

### DX12 GPU レンダリング

DirectX 12 ネイティブ GPU レンダラーにより、大量テキスト出力時でも高速な描画を実現。HLSL シェーダーによるグリフレンダリングパイプラインを搭載しています。

### Intelligence モジュール

`zwg-intelligence` クレートにより、Flash Attention・投機的デコーディング・KV 量子化・PagedAttention などの AI 高速化技術を統合。Ollama との連携で、ローカル LLM 推論を大幅に高速化します。

### tmux 互換 IPC

Windows Named Pipes ベースの tmux 互換 IPC レイヤーを内蔵。Claude Code の TeamCreate によるエージェントチームを自動的にペイン分割で可視化します。

### WASM ランタイム

アプリ起動時に埋め込み WASM モジュールを安全に初期化し、`zig-ffi` / `dx12-renderer` / `gpui-host` の各 capability を検証します。ホスト import を持たないため、ファイル・ネットワーク・Windows API への直接アクセスはできません。

### Win32 ネイティブ統合

- **カスタムタイトルバー**: Win32 `PostMessageW(WM_SYSCOMMAND, SC_MOVE)` による直接ドラッグ
- **ワークエリア制約**: `MonitorFromWindow` + `GetMonitorInfoW` でタスクバーを考慮した配置
- **ConPTY**: Windows Pseudo Console API によるネイティブなシェル実行

### メモリアロケータ

`mimalloc` を使用し、高頻度の小規模アロケーション（ターミナル出力バッファ等）を高速化しています。

---

## インストーラー

GUI インストーラー (`ZWG_Terminal_Setup.exe`) を使えばビルド不要でインストールできます。

### インストーラーの機能

- インストール先の選択
- デスクトップショートカットの作成
- スタートメニューへの追加
- Windows「プログラムの追加と削除」への登録
- アンインストーラーの同梱

### インストーラーのビルド

```bash
# 1. リリースビルド
cargo zigbuild --release -p zwg-app

# 2. インストーラー作成（ステージング + PyInstaller）
python installer/build_installer.py
# → installer/dist/ZWG_Terminal_Setup.exe が生成されます
```

## MSIX / 署名 / Winget

### `.msix` ビルド

```powershell
# リリース EXE を先に作成
cargo zigbuild --release -p zwg-app

# 自己署名証明書を作成
powershell -ExecutionPolicy Bypass -File packaging/windows/New-CodeSigningCert.ps1

# MSIX を生成して署名
powershell -ExecutionPolicy Bypass -File packaging/windows/Build-MSIX.ps1 `
  -Version 1.3.0.0 `
  -Publisher "CN=ZWG Terminal Test" `
  -Architecture x64 `
  -PfxPath packaging/windows/certs/ZWGTerminal-TestCert.pfx `
  -PfxPassword changeit
```

### `.exe` ビルド

```powershell
# ポータブル EXE
cargo zigbuild --release -p zwg-app

# GUI セットアップ EXE
python installer/build_installer.py
```

### コード署名手順（自署名）

```powershell
# 1. 証明書作成
powershell -ExecutionPolicy Bypass -File packaging/windows/New-CodeSigningCert.ps1 `
  -Subject "CN=ZWG Terminal Test" `
  -Password "changeit"

# 2. EXE 署名
signtool sign /fd SHA256 /f packaging/windows/certs/ZWGTerminal-TestCert.pfx `
  /p changeit /tr http://timestamp.digicert.com /td SHA256 target/release/zwg.exe

# 3. MSIX 署名
signtool sign /fd SHA256 /f packaging/windows/certs/ZWGTerminal-TestCert.pfx `
  /p changeit /tr http://timestamp.digicert.com /td SHA256 `
  packaging/windows/dist/ZWG_Terminal_1.3.0.0_x64.msix
```

### Winget / Microsoft Store 提出物

- Winget manifest テンプレート: `packaging/winget/manifests/s/sayasaya8039/ZWGTerminal/1.3.0/`
- Microsoft Store / sideload 用 AppxManifest: `packaging/windows/AppxManifest.xml`
- 本体 EXE の実行レベルは `resources/windows/app.manifest` で `asInvoker` を固定

### ARM64 確認

```powershell
# ARM64 向けに Zig と Rust の両方を検証
rustup target add aarch64-pc-windows-msvc
cargo build --release --target aarch64-pc-windows-msvc
powershell -ExecutionPolicy Bypass -File packaging/windows/Build-MSIX.ps1 `
  -Version 1.3.0.0 `
  -Publisher "CN=ZWG Terminal Test" `
  -Architecture arm64
```

---

## ライセンス

MIT License

Copyright (c) 2026 ZWG Terminal contributors
