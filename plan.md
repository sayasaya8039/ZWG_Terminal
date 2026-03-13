# Ghostty Windows 移植プロジェクト Plan

**バージョン**: 1.0 (2026年3月策定)  
**プロジェクト名**: Ghostty-Windows (仮称: `ghostty-win`)  
**リポジトリ**: https://github.com/ghostty-org/ghostty をフォークし、`windows/` ブランチで開発  
**目標**: macOS/Linux 版 Ghostty の全機能を Windows で再現し、**Zig + GPUI + WASM + NPU** を駆使した「世界最速・最軽量」Windows ネイティブ端末エミュレーターを実現。  
**対応シェル**: PowerShell、PowerShell 7、CMD、WSL、Git Bash（すべてネイティブ ConPTY 経由）

---

## 1. 詳細なプロジェクト計画（Phased Roadmap）

### Phase 0: 準備（2週間）
- 公式 Ghostty リポジトリ解析（`libghostty-vt` が既に Windows ターゲット対応済み）
- gpui-ghostty POC（https://github.com/Xuanwo/gpui-ghostty）をベースにフォーク
- 開発環境構築：
  - Zig 0.14+（core）
  - Rust + GPUI（Zed pinned commit）
  - Windows SDK + ConPTY
  - CUDA/DirectML/NPU ドライバ（オプション）
- CI/CD: GitHub Actions（Windows 11 VM + Zig build + Rust test）

### Phase 1: Core 移植（4週間）
- `src/pty.zig` + `src/termio.zig` に Windows ConPTY バックエンド追加（公式 #2563 議論を参照）
- `src/os/windows.zig` 新規作成（ファイルハンドル、シグナル、フォントパス）
- libghostty C API を Windows DLL としてビルド（`build.zig` 拡張）
- WASM 版 `libghostty-vt` を強化（将来的に Web 版ターミナル用）

### Phase 2: GPUI Frontend 構築（6週間）
- GPUI + Ghostty VT 統合（gpui-ghostty を本格化）
- 独自 Renderer 作成（GPUI の WebGPU/Vulkan バックエンドを活用 → Metal 並みの 60fps 保証）
- NPU アクセラレーション層追加（DirectML + ONNX Runtime でコマンド予測・シンタックスハイライト前計算）
- シェルランチャー実装（5種類すべて対応）

### Phase 3: 機能完成・最適化（8週間）
- フル機能ポート（タブ/スプリット/リガチャ/キットグラフィックスプロトコル）
- パフォーマンスチューニング（Zig SIMD + NPU オフロード）
- 設定 GUI（GPUI ネイティブ Preference Panel）
- パッケージング（.msi + winget + Chocolatey）

### Phase 4: テスト・リリース（4週間）
- xterm 準拠テスト（公式テストスイート全通過）
- 実機テスト（PowerShell 7 / WSL2 / Git Bash）
- 1.0 リリース（GitHub Release + 公式 Discord 告知）

**総開発期間**: 約 5ヶ月（フルタイム1名 + コントリビューター想定）

---

## 2. チーム構成（推奨）

| 役割                  | 人数 | 必須スキル                              | 担当フェーズ          |
|-----------------------|------|-----------------------------------------|-----------------------|
| **Project Lead**     | 1    | Zig, Rust, プロジェクト管理            | 全フェーズ            |
| **Zig Core Engineer** | 1-2  | Zig, PTY/ConPTY, libghostty-vt         | Phase 1, 3            |
| **GPUI/Rust Engineer**| 1-2  | Rust, GPUI (Zed), WebGPU               | Phase 2, 3            |
| **NPU/AI Specialist** | 1    | DirectML, ONNX, Windows AI             | Phase 2（オプション）|
| **UI/UX Designer**   | 1    | Figma + Fluent Design                  | Phase 2-3             |
| **Tester / QA**      | 2    | Windows 11, PowerShell/WSL 熟練        | Phase 4               |
| **Community Manager**| 1    | Discord/GitHub                         | 全フェーズ            |

- **初期コアチーム**: あなた（Lead）＋ Zig エンジニア1名＋Rust エンジニア1名でスタート可能
- コントリビューター募集：Ghostty Discord / Reddit r/Ghostty / X で告知

---

## 3. 実装構造（モジュール構成）
ghostty-win/
├── src/
│   ├── core/                  # Zig 共有コア（公式そのまま + Windows 拡張）
│   │   ├── libghostty-vt/     # VT パーサー・状態管理（WASM 対応済み）
│   │   ├── pty/               # ConPTY + winpty フォールバック
│   │   ├── renderer/          # Zig SIMD 高速描画（GPUI に供給）
│   │   └── os/windows.zig
│   ├── frontend/              # Rust + GPUI（新設）
│   │   ├── gpui_app.rs        # メインアプリケーション
│   │   ├── terminal_view.rs   # Ghostty VT + GPUI カスタム描画
│   │   ├── npu/               # NPU アクセラレーション層
│   │   └── shell/             # 5種類シェルランチャー
│   └── wasm/                  # WASM 版（将来的な Web ターミナル用）
├── build.zig                  # Zig ビルド（Windows DLL 生成）
├── Cargo.toml                 # Rust GPUI 依存
├── resources/                 # アイコン・Fluent Design アセット
└── packaging/                 # .msi / winget スクリプト
text- **キー抽象化**:

  - `apprt` モジュールに `windows.rs`（GPUI バックエンド）を追加
  - Renderer trait を拡張（Metal → GPUI WebGPU）
  - 設定は TOML（公式互換）＋ GPUI ネイティブ GUI

---

## 4. 実装機能（Ghostty 公式 + Windows 拡張）

### 必須機能（Phase 2 完了時）
- 完全 xterm 準拠 + ligature + Kitty Graphics Protocol
- タブ / スプリットペイン / マルチウィンドウ
- GPU 60fps 描画（GPUI WebGPU）
- フォント：DirectWrite + HarfBuzz（公式互換）
- シェル起動：
  - `powershell.exe`
  - `pwsh.exe`（PowerShell 7）
  - `cmd.exe`
  - `wsl.exe --distribution Ubuntu`
  - `git-bash.exe`（MINGW）
- ConPTY フルサポート（Windows 10 1903+）
- クリップボード / ドラッグ&ドロップ / ハイパーリンク

### 高速化特化機能（Zig/WASM/GPUI/NPU）
- **Zig**: コアロジック全般（SIMD テキスト処理）
- **WASM**: 軽量 VT パーサー（将来的に Electron 不要 Web 版）
- **GPUI**: ネイティブ GPU UI（Zed 並みの滑らかさ）
- **NPU**: 
  - コマンド予測（ローカル LLM 推論）
  - シンタックス前計算
  - スクロールバック圧縮（AI ベース）

### 拡張機能（Phase 3）
- Fluent Design 準拠（ Mica / Acrylic エフェクト）
- 設定 GUI（Preference Panel）
- クラッシュレポート（Windows Event Log + Sentry）
- winget / Chocolatey / Microsoft Store 配布

---

## 5. UI 設計（Fluent Design + Ghostty 精神）

- **全体テーマ**: 「macOS 並みの美しさ × Windows Fluent ネイティブ感」
- **ウィンドウ**:
  - Mica タイトルバー（透明効果）
  - 角丸 + アクリル背景（オプション）
- **タブバー**: GPUI ネイティブタブ（macOS 風 + Fluent スタイル切り替え）
- **スプリット**: ドラッグで自由分割（公式と完全互換）
- **コマンドパレット**: `Ctrl+Shift+P`（Zed 風）
- **設定画面**: ネイティブ Preference（フォント/色/シェル選択）
- **ステータスバー**: シェル種別・CPU/GPU 使用率（NPU 負荷も表示）
- **ダーク/ライト**: Windows システム設定に完全連動

**モックアップイメージ**（Figma で作成予定）:
- メイン画面：PowerShell 7 + スプリット + タブ
- 設定画面：Fluent スタイル

---

## リスクと対策

- **Zig + Rust FFI**: gpui-ghostty POC で既に実証済み → 即採用
- **ConPTY 互換性**: Windows 10 1903 未満は winpty フォールバック
- **NPU 対応**: オプション機能（NPU なしでもフル動作）
- **パフォーマンス**: ベンチマークを毎週実施（`cat bigfile.txt` で iTerm/WezTerm 超えを目指す）

---

