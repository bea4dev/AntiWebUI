# AntiWebUI

WebView に頼らず、**wgpu による自前描画 + Rust** で UI を組む軽量フレームワークです。

- **fractional scaling を最初から正しく**: スタイルは論理ピクセルで書き、レイアウトは物理整数ピクセルで行います。125% や 180% でも枠線がにじまず、同じ指定の要素はどこに置いても同じ大きさになります。
- **ブラウザ並みのテキスト選択**: 複数の要素にまたがるドラッグ選択、ダブル/トリプルクリック、コピー、primary selection。
- **IME 対応**: Wayland text-input-v3、X11 の XIM、macOS の `NSTextInputClient`、Windows の IMM32。未確定文字列をインラインで表示します。
- **SolidJS 風**: `div().flex().gap(8.0)` のビルダーと `signal()` による細粒度の差分更新。

設計は [docs/design](docs/design/README.md) にあります。

## 例

```rust
use anti_webui::prelude::*;

fn main() {
    App::new().title("Counter").run(|| {
        let count = signal(0);
        div()
            .flex().flex_col().gap(8.0).p(16.0)
            .child(text(move || format!("Count: {}", count.get())))
            .child(button("+1").on_click(move |_| count.update(|c| *c += 1)))
    }).unwrap();
}
```

```sh
cargo run -p anti-webui --example showcase
cargo run -p anti-webui --example widgets
# 開発中は Bevy と同じく動的リンクでビルドを速くできます
cargo run -p anti-webui --example showcase --features dynamic_linking
```

## クレート

| クレート | 役割 |
|---|---|
| `anti-webui` | ファサード（`dynamic_linking` feature） |
| `anti-webui-core` | リアクティブ・ノードツリー・スタイル解決・レイアウト・テキスト・選択・編集・入力処理 |
| `anti-webui-style` | スタイル型と `Styled` ビルダートレイト |
| `anti-webui-widgets` | ボタン、入力、メニュー、ダイアログ、テーブルなどの標準の部品とテーマ |
| `anti-webui-wgpu` | レンダラ |
| `anti-webui-video` | 動画のデコードと音声。実行時に読み込む FFmpeg を使い、FFmpeg のコードは含まない（`video` feature） |
| `anti-webui-wayland` | Wayland バックエンド |
| `anti-webui-x11` | X11 バックエンド |
| `anti-webui-macos` | macOS バックエンド（AppKit） |
| `anti-webui-windows` | Windows バックエンド（Win32） |
| `anti-webui-app` | 上記の結線、オフスクリーン描画 |

## 状態

M1（Wayland 上の基本 UI）が動作する段階です。対応 OS は Linux（Wayland、X11）、macOS（[38-platform-macos](docs/design/38-platform-macos.md)）、Windows（[39-platform-windows](docs/design/39-platform-windows.md)）です。ロードマップは [07-roadmap](docs/design/07-roadmap.md) を参照してください。

## ライセンス

MIT License または Apache License 2.0 のどちらかを選べます（[LICENSE-MIT](LICENSE-MIT)、[LICENSE-APACHE](LICENSE-APACHE)）。作者は bea4dev です。

`anti-webui-symbols-outlined` は、同梱するフォントに合わせて Apache License 2.0 のみです。動画は FFmpeg を実行時に読み込むだけで、FFmpeg のコードは含みません（[24-video](docs/design/24-video.md)）。
