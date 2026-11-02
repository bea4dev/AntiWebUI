# AntiWebUI

WebView に頼らず、**wgpu による自前描画 + Rust** で UI を組む軽量フレームワークです。

- **fractional scaling を最初から正しく**: スタイルは論理ピクセルで書き、レイアウトは物理整数ピクセルで行います。125% や 180% でも枠線がにじまず、同じ指定の要素はどこに置いても同じ大きさになります。
- **ブラウザ並みのテキスト選択**: 複数の要素にまたがるドラッグ選択、ダブル/トリプルクリック、コピー、primary selection。
- **IME 対応**: Wayland text-input-v3。未確定文字列をインラインで表示します。
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
# 開発中は Bevy と同じく動的リンクでビルドを速くできます
cargo run -p anti-webui --example showcase --features dynamic_linking
```

## クレート

| クレート | 役割 |
|---|---|
| `anti-webui` | ファサード（`dynamic_linking` feature） |
| `anti-webui-core` | リアクティブ・ノードツリー・スタイル解決・レイアウト・テキスト・選択・編集・入力処理 |
| `anti-webui-style` | スタイル型と `Styled` ビルダートレイト |
| `anti-webui-widgets` | Button / Checkbox / Switch / Slider / TextField など |
| `anti-webui-wgpu` | レンダラ |
| `anti-webui-wayland` | Wayland バックエンド |
| `anti-webui-app` | 上記の結線、オフスクリーン描画 |

## 状態

M1（Wayland 上の基本 UI）が動作する段階です。対応 OS は現在 Linux (Wayland) のみです。ロードマップは [07-roadmap](docs/design/07-roadmap.md) を参照してください。
