# 27. フルスクリーン

ウィンドウを画面全体に広げることと、その中の 1 つの要素を画面全体に出すことの 2 段を用意する。どちらも `core` の機能で、PDF のプレゼンテーションモードや動画はこの上に作る。

## API

```rust
// ウィンドウごと。F11 に当たる。UI はそのままで、広くなるだけ
set_window_fullscreen(true);
window_fullscreen();              // 読むと追跡される。WM の操作で変わったときも追う

// 要素だけ。Web の element.requestFullscreen() に当たる
let node = node_ref();
node.request_fullscreen();
node.is_fullscreen();             // 読むと追跡される
exit_fullscreen();                // document.exitFullscreen()

// 最初からフルスクリーンで開く
App::new().fullscreen(true)
```

`cursor: none` に当たる `CursorIcon::None` も足す。

## 要素のフルスクリーンの決まり

CSS の Fullscreen API と同じ見え方にする。

| 項目 | 決まり | 同じもの |
|---|---|---|
| 大きさ | 要素の `width`、`height`、`margin` に関係なくウィンドウ全体を占める | UA スタイルの `:fullscreen` |
| 祖先の影響 | 祖先の `overflow`、`transform`、`opacity` を受けない | top layer |
| 背景 | 要素が塗らないところは黒 | `::backdrop` |
| その他の要素 | 描かれず、当たり判定も受けない | 全面を覆うのと同じ |
| ポップアップ | 要素の上に出る | top layer の後から出たもの |
| イベント | キーやポインタは DOM と同じく祖先へ伝わる | 同じ |
| Esc | 既定の動作で抜ける。開いているメニューがあれば先に閉じる。`prevent_default` で止められる | ブラウザは止められないが、アプリが自分で決められるようにする |
| ウィンドウ | 通常ならフルスクリーンにして、抜けるときに戻す。F11 で既にフルスクリーンならそのまま | 同じ |

### フォーカス

要素の外にあるフォーカスは、隠れたまま文字を受けてしまう。そこで入るときに要素へ移し、抜けるときに戻す。フルスクリーンの間は、外のノードへのフォーカスと Tab の移動を無視する。

## 描画と配置

ブラウザのように `position: fixed` 扱いにして流れから外すと、周りが詰まって配置し直しになる。スクロール位置も縮んだ分だけ失われる。これは Web でよく問題になるところで、ここでは避ける。

**フルスクリーンの間は、要素を根として扱い、元の木の配置を止める。**

- `layout_document` と `position_document` は要素から始める。大きさはウィンドウと同じにする。
- 元の木の `layout_dirty` と `position_dirty` は残るので、抜けたときに変わったところだけ計算し直す。
- 抜けるときは、要素から根までを無効にする。ウィンドウの大きさが元に戻れば、それ以外はキャッシュのままで済み、スクロール位置も変わらない。
- 描画と当たり判定も、根の代わりに要素から始める。元の木を描かないので、後ろに隠れたものの分だけ軽い。
- 要素は重ね合わせの文脈になる。子孫の `absolute` や `z_index` は要素の中で完結する。
- フルスクリーンの間に元の木で作られたノードは、配置されないまま隠れている。`NodeRef::rect` や `on_resized` は抜けるまで更新されない。

## 切り替えの順番

画面の大きさが変わるのと中身が変わるのを同じフレームにするため、ウィンドウの状態が変わるのを待つ。

1. `request_fullscreen` は要素を覚えて `PlatformRequest::SetFullscreen(true)` を出す。
2. コンポジタの configure で `WindowEvent::Fullscreen(true)` と `Resized` が届き、次のフレームでまとめて反映する。
3. `exit_fullscreen` は `SetFullscreen(false)` を出すだけで、`Fullscreen(false)` が届いてから要素を解除する。

WM のキー操作でフルスクリーンが解けたときも `Fullscreen(false)` が届くので、同じ道を通って要素も解除する。

`WindowEvent::Fullscreen` は状態が変わったときだけ届く。コンポジタが断ると何も起きず、要求は残る。次の `exit_fullscreen` か `request_fullscreen` で消える。

## Wayland

- `xdg_toplevel.set_fullscreen(NULL)` で、出力はコンポジタに任せる。
- `configure` の `states` から `fullscreen` を読み、`xdg_surface.configure` で確定する。大きさと同じ時点で反映するためである。
- 抜けたときの `configure` は大きさが 0 × 0 のことがある。その場合はフルスクリーン前の大きさに戻す。

### スキャンアウト

フルスクリーンのバッファをコンポジタが合成せず直接画面に出せれば、コピーが 1 回減り、遅延と電力が下がる。`zwp_linux_dmabuf_feedback_v1` は、それができるときに `scanout` 付きの tranche を先頭で送ってくる。

- 今は全 tranche の修飾子をまとめてドライバに選ばせているので、スキャンアウトできない修飾子になることがある。
- **先頭の tranche が `scanout` 付きで、その修飾子のどれかで描けるなら、それだけを候補にする。** 描けなければ今までどおりにする。
- feedback が変わっても、今のバッファの修飾子が新しい候補にあれば作り直さない。ない場合だけ作り直し、古いバッファはコンポジタが返却してから壊す（06 の「バッファ」を参照）。
- 形式は ARGB8888 のままとし、不透明であることは `set_opaque_region` で伝える。

ShojiWM で確かめたところ、フルスクリーンの間は feedback の先頭が `scanout` 付きになり、候補の修飾子が 9 個から 5 個に絞られた（2026-11-06、`ANTIWEBUI_DEBUG_DAMAGE=1` で出る）。コンポジタが実際に直接画面に出したかどうかは、こちらからは確かめられていない。

## 後回しにするもの

- 出力を指定したフルスクリーン（複数画面）
- `:fullscreen` に当たる疑似クラス。`dyn_style` と `is_fullscreen()` で同じことができる
- `::backdrop` の色の指定
- X11、Windows、macOS（バックエンド自体がまだ）
