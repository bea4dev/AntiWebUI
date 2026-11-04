# AntiWebUI 設計ドキュメント

AntiWebUI は WebView に頼らず、wgpu による自前描画とネイティブ言語で UI を組むための軽量フレームワークです。

| # | ドキュメント | 内容 |
|---|---|---|
| 00 | [overview](00-overview.md) | 目標・クレート構成・リンク方式・多言語バインディング方針 |
| 01 | [pixel-model](01-pixel-model.md) | 物理整数ピクセルモデル（fractional scaling の扱い） |
| 02 | [reactivity-and-view](02-reactivity-and-view.md) | SolidJS 風リアクティブシステムとビュー構築 API |
| 03 | [layout](03-layout.md) | 自前 Flex レイアウトと整数分配アルゴリズム |
| 04 | [text](04-text.md) | テキスト整形・ドキュメント横断選択・編集・IME |
| 05 | [rendering](05-rendering.md) | wgpu レンダラ |
| 06 | [platform-wayland](06-platform-wayland.md) | Wayland バックエンド |
| 07 | [roadmap](07-roadmap.md) | マイルストーン |
| 08 | [scrolling-and-settings](08-scrolling-and-settings.md) | 慣性スクロールと全体設定 |
| 09 | [shadows-stacking-popups](09-shadows-stacking-popups.md) | 影、z_index、ポップアップ（xdg_popup / wl_subsurface） |
| 10 | [frame-pacing](10-frame-pacing.md) | フレームの先行（トリプルバッファリング）とフレーム計測 |
| 11 | [gpu-fill](11-gpu-fill.md) | GPU の塗りの効率（単色の経路と箱の分割、Chromium との比較） |
| 12 | [transform](12-transform.md) | transform と合成レイヤー（カスタムシェーダーへの備えを含む） |
| 13 | [animation](13-animation.md) | `transition`、`animated`、`Spring`、`enter` / `exit`、リアクティブの計算量 |
| 14 | [shaders](14-shaders.md) | カスタムシェーダー（`fill`、`overlay`） |
| 15 | [md3e](15-md3e.md) | Material 3 Expressive の部品（`anti_webui_md3e`） |
| 16 | [filters](16-filters.md) | `filter`、ブラー、backdrop、入力を読むシェーダー |
| 17 | [font-axes](17-font-axes.md) | 可変フォントの軸の指定とアニメーション |

## 用語

| 用語 | 意味 |
|---|---|
| 論理ピクセル (logical px) | ユーザーがスタイルに書く単位。`f32` |
| 物理ピクセル (physical px) | バッファ上の実ピクセル。レイアウト結果は `i32` |
| スケール | `n / 120`（Wayland `wp_fractional_scale_v1` と同じ表現）。`Scale(n)` |
| ノード | 保持型 UI ツリーの要素（DOM ノード相当） |
| オーナー | リアクティブ計算・ノードの寿命を管理するスコープ |
