# 00. 概要

## 目標

1. **軽量**: Chromium を同梱しない。必要な機能だけを Cargo feature / 別クレートで足す。
2. **fractional scaling を最初から正しく**: 125% / 150% などでも境界がにじまず、要素サイズが位置によってぶれない（[01](01-pixel-model.md)）。
3. **ブラウザ並みのテキスト**: 複数要素をまたぐ選択、日本語 IME、双方向テキスト、フォントフォールバック（[04](04-text.md)）。
4. **SolidJS 風の書き味**: `div().flex().gap(8.0)` のビルダーと `signal(0)` による細粒度の差分更新（[02](02-reactivity-and-view.md)）。
5. **多言語**: Rust で実装し、C ABI を介して C++ / C# にバインディングを提供する。

Web 技術の資産そのもの（HTML/JS）の流用は目標にしない。代わりに Flexbox・CSS 風スタイル・インスペクタ・ホットリロードなど「Web の考え方」を借りる。

## レイヤー構成

```
┌──────────────────────────────────────────────────────────────┐
│ anti-webui            ファサード（Bevy 方式の dynamic_linking）  │
│ anti-webui-internal   全クレートの再エクスポート                 │
│ anti-webui-dylib      開発用 Rust dylib                         │
├──────────────────────────────────────────────────────────────┤
│ anti-webui-app        プラットフォーム × レンダラ × core の結線   │
│ anti-webui-widgets    標準の部品（21-widgets.md）               │
│ anti-webui-md3e       Material 3 Expressive の部品（任意）       │
├──────────────────────────────────────────────────────────────┤
│ anti-webui-core       リアクティブ・ノードツリー・スタイル解決・   │
│                       レイアウト・テキスト・選択・編集・イベント   │
│ anti-webui-style      スタイル型（論理値）・将来の CSS サブセット  │
│ anti-webui-macros     `derive(ShaderParams)`                    │
├──────────────────────────────────────────────────────────────┤
│ anti-webui-wgpu       描画リスト → GPU（矩形 SDF・グリフアトラス）│
│ anti-webui-wayland    Wayland バックエンド                      │
│ anti-webui-x11        X11 バックエンド                          │
│ anti-webui-macos      macOS バックエンド（AppKit）               │
│ anti-webui-windows    Windows バックエンド（Win32）             │
└──────────────────────────────────────────────────────────────┘
```

依存の向き:

- `core` は GPU も OS も知らない。入力は「プラットフォーム非依存のイベント」、出力は「描画リスト（Scene）」と「プラットフォームへの要求（カーソル形状、IME 状態、クリップボード）」。
- `wgpu` は `core::scene` だけを読む。
- `wayland` は `core::event` と `core::platform` の型だけを使い、wgpu を知らない（`raw-window-handle` を提供するだけ）。
- `app` がこれらを結線してイベントループを回す。

この分離により、将来 C ABI を被せるときに「core だけをラップすればよい」状態を保つ。

## リンク方式

| 成果物 | crate-type | 用途 | ABI 安定性 |
|---|---|---|---|
| Rust 静的リンク | `rlib` | 通常の Rust 利用・リリースビルド | — |
| Rust 動的リンク | `dylib`（`anti-webui-dylib`） | 開発時のリンク時間短縮 | なし（同一コンパイラ・同一フラグ必須） |
| C ABI | `cdylib` + `staticlib`（`anti-webui-ffi`、将来） | C++ / C# 向け配布 | あり |

Rust の動的リンクは Bevy と同じ構成を取る:

```toml
# anti-webui/Cargo.toml
[features]
dynamic_linking = ["dep:anti-webui-dylib"]
```

```rust
// anti-webui/src/lib.rs
#[cfg(feature = "dynamic_linking")]
#[allow(unused_imports)]
use anti_webui_dylib;
pub use anti_webui_internal::*;
```

`anti-webui-dylib` は `crate-type = ["dylib"]` で `anti-webui-internal` に依存するだけのクレート。feature を有効にすると依存ツリー全体が 1 つの `.so` にまとまり、ユーザーコード変更時のリンクが速くなる。配布用途ではない。

## 多言語バインディング方針（将来）

```
 Rust 宣言的 API │ C++ RAII ラッパ │ C# ラッパ     ← 各言語らしい薄い層
 ────────────────┴─────────────────┴──────────
        C ABI  (anti-webui-ffi)                   ← 唯一の公開境界
 ─────────────────────────────────────────────
  core: ノードツリー（DOM 的な命令型 API）+ リアクティブグラフ
```

- core の API は「ノードを作る / プロパティを設定する / 子を追加する / コールバックを登録する」という **命令型** の形で設計する。宣言的な書き味は各言語側の層で作る。
- リアクティブグラフ（signal / effect）は core に置き、C ABI 越しに C++ / C# からも同じ仕組みを使えるようにする。
- C ABI のルール: ポインタではなく世代付きハンドル、文字列は UTF-8 の `(ptr, len)`、コールバックは `fn(user_data, event)`、UI スレッド限定。
- ツール: C ヘッダは `cbindgen`、C# は `csbindgen` を第一候補にする。C# は NativeAOT + `staticlib` での単一バイナリ配布も可能。

現在の Rust API も、内部的にはこの命令型 core の上に乗るように実装する（ビルダーはノード生成と effect 登録の糖衣）。
