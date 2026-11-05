# 22. 画像

`<img>` と `background-image` にあたるものを作る。CSS でできることは同じ書き方で同じ結果にする。効率は Chromium に合わせる。特に、メモリは「ファイルの大きさ」ではなく「画面に出す大きさ」に比例させる。

```rust
// <img src="photo.jpg" style="width: 200px; height: 150px; object-fit: cover; border-radius: 12px">
img("photo.jpg").w(200.0).h(150.0).object_fit(ObjectFit::Cover).rounded(12.0)

// 埋め込み、SVG、切り替え
img(include_bytes!("logo.svg"))
img(move || if dark.get() { "dark.png" } else { "light.png" })

// background-image（hover やシートでも切り替えられる）
div()
    .bg_image("hero.webp")
    .bg_size(BackgroundSize::Cover)
    .bg_position(pct(50.0), pct(50.0))
    .child(text("タイトル"))
```

## API

`Image` は `filter` の式（[16](16-filters.md)）で使っているので、要素は HTML に合わせて `img` にする。

| API | CSS / HTML | 備考 |
|---|---|---|
| `img(src)` | `<img src>` | `src` は `ImageSource` になるものか、それを返すクロージャ |
| `ImageSource` | | パス、`&'static [u8]`、`Vec<u8>` / `Arc<[u8]>`、`ImageSource::rgba(w, h, pixels)` |
| `.object_fit(ObjectFit)` | `object-fit` | `Fill`（既定）/ `Contain` / `Cover` / `None` / `ScaleDown` |
| `.object_position(x, y)` | `object-position` | `pct()` か `px()`。既定は `pct(50.0), pct(50.0)` |
| `.aspect_ratio(r)` | `aspect-ratio` | すべての要素で使える |
| `.image_rendering(ImageRendering)` | `image-rendering` | `Auto` / `Pixelated`。継承する |
| `.density(d)` | `srcset="a.png 2x"` | 画像の 1 px を論理 `1/d` px として大きさを決める |
| `.image_opacity(a)` | | 画像だけの透明度。CSS にはない。背景の色を残して画像をフェードインするために使う（`Prop::ImageOpacity` で遷移できる） |
| `.bg_image(src)` | `background-image` | スタイルなので `.hover()` やシートでも使える |
| `.bg_size(BackgroundSize)` | `background-size` | `Auto` / `Cover` / `Contain` / `Size(w, h)` |
| `.bg_position(x, y)` | `background-position` | `pct()` か `px()` |
| `.bg_repeat(BackgroundRepeat)` | `background-repeat` | `Repeat`（既定）/ `RepeatX` / `RepeatY` / `NoRepeat` |
| `.on_load(f)` | `load` | 実寸が分かったとき。`f(width, height)` は論理 px |
| `.on_decoded(f)` | `decode()` の完了 | 表示できる画素が初めて届いたとき |
| `.on_error(f)` | `error` | |
| `.child(view)` | | 画像の箱の中に重ねる。キャプションや読み込み中の表示に使う |

角丸、`opacity`、`filter`、`transform`、影、枠線は他の要素と同じように効く。`Style` の中に `ImageSource` を持つので、`bg_image` は `dyn_style`、`hover`、`Sheet` のどこからでも指定できる。

`src` が変わったとき、新しい画像が届くまでは古い画像を表示する。ブラウザの `<img>` と同じ動きである。

## 大きさ

`<img>` と同じ置換要素として扱う。

- 実寸は画像の px を論理 px とみなしたもの（`density` で割る）。EXIF の向きを適用した後の大きさを使う。
- 幅と高さが両方 `auto` なら実寸。片方が決まっていれば、もう片方は `aspect-ratio` か実寸の比から決める。`min-*` / `max-*` は比を保って解決する（CSS 2.1 の表どおり）。
- 実寸が分かるまでは 0×0。届いたらレイアウトをやり直す。`width` と `height` を書いておけばずれない。
- ブロックの中でも幅いっぱいに伸びない。フレックスでは `align-items: stretch` で伸びる。これもブラウザと同じである。
- `aspect_ratio` は一般の要素では「自動の側の最小値」として働き、中身の方が大きければ箱が伸びる。

## 読み込みの流れ

```
UI スレッド                         ワーカー
img("a.jpg") を作る ───────────────→ ヘッダだけ読む（幅、高さ、向き）
         ←────────────── 実寸 ───── 
レイアウト、ペイント
ペイントが「この大きさが要る」と言う ─→ 全体をデコード
                                     向きを適用、ICC を sRGB に変換
                                     乗算済みアルファにする
                                     必要な大きさまで縮小、ミップを作る
         ←────────────── 画素 ───── 
次のフレームで表示（テクスチャへの転送）
```

UI スレッドのブロックはファイルの読み込みにもデコードにも起こらない。スレッドはアプリの作者から見えない（[UI スレッドは 1 本](05-rendering.md)の方針）。使うコア数は `min(コア数 - 1, 4)`、仕事がなくなって 10 秒で終わる。

### Chromium との対応

| | Chromium | AntiWebUI |
|---|---|---|
| ヘッダの解析 | 読み込み後すぐ | 同じ（ワーカー） |
| デコードの開始 | そのタイルをラスタするとき。画面外は遅延 | ペイントが要求したとき。カリングされた画像はデコードしない。マージンは画面 1 枚分（既存のレイヤーのカリングと同じ） |
| デコードの大きさ | `GpuImageDecodeCache` が描画スケール以上で最小のミップに縮小してから転送 | 同じ考え方。描画先以上で最小の 1/2^n まで縮小し、そこからミップを作って転送 |
| JPEG の DCT スケーリング | あり | なし（`zune-jpeg` が対応していない）。全体をデコードしてから縮小する（12 MP で約 70 ms） |
| 色 | ICC を出力の色空間に変換 | ICC を sRGB に変換 |
| 向き | `image-orientation: from-image` | 同じ（常に適用） |
| GPU のサンプリング | 箱以上で最小のレベルを線形で読む（実測） | 同じ |
| キャッシュの予算 | 作業セット約 128 MB + 上限 | 256 MB。画面に出ているものは予算を超えても残す |
| 転送後の CPU 側の画素 | discardable として解放可能に | 転送後に解放し、必要になったらデコードし直す |
| アニメーション | フレームごとにデコード、画面外は止める | デコード済みのフレームを持つ（下記）。画面外は止める |
| SVG | 描画命令をタイルにラスタ | ワーカーで描画先の大きさにラスタ（`resvg`） |

### 必要な大きさ

ペイントは画像を置く矩形と、そこまでの変形の倍率から「必要な物理 px」を求める。

```
必要な px = 表示する矩形の物理 px × 祖先の transform の倍率
level     = 画像を半分にしても必要な px を下回らない最大の回数
```

1 つの画像につき保持するビットマップは 1 枚で、一番大きく要求されたものに合わせる。小さく要求された側はそのミップを読む。後からもっと大きく要求されたら（ウィンドウの拡大、`scale` のアニメーション）デコードし直し、届くまでは今あるものを引き伸ばして表示する。

`image-rendering: pixelated` は縮小しない（常に等倍）。

SVG は半分ずつではなく、要求された大きさちょうどにラスタする（形をノードとして持つ `svg` は [25](25-svg.md)）。文字やアイコンの輪郭をにじませないためである。大きさごとに 1 枚持ち、シーンから消えたものから捨てる。

### 1 フレームに転送する量

デコードが終わった画素をそのフレームに全部送ると UI スレッドが止まるので、1 フレームあたり 64 MB までにして、残りは次のフレームに回す。この調整はコア側で完結するので、レンダラと差分描画は「シーンに載っている画像はそのフレームで描ける」と考えてよい。

### キャッシュ

- キーは `ImageSource` の同一性（パスの中身、静的スライスのアドレス、`Arc` のアドレス）。
- ビットマップの合計が 256 MB を超えたら、どのシーンからも参照されていない（`Arc` の参照がキャッシュだけ）ものを、最後に使った順に捨てる。
- `update_settings(|s| s.images.cache_bytes = ...)` で変えられる。
- 捨てたものはまた必要になればデコードし直す。パスは読み直す。
- 要素がなくなり、ビットマップも持たないエントリはすぐ消す。

## レンダラ

```rust
Primitive::Image(ImageDraw {
    bitmap: Arc<Bitmap>,
    frame: u32,        // アニメーションの何枚目か
    rect: RectI,       // 画像全体（繰り返しなら 1 枚分）を置く矩形
    repeat: (bool, bool),
    shape: Clip,       // 自分の形。<img> はコンテンツボックスの内側の角丸
    clip: Clip,        // 祖先のクリップ
    opacity: f32,
    pixelated: bool,
})
```

- テクスチャは画像ごとに 1 枚（`Rgba8Unorm`、ミップあり）。Chromium も画像ごとに持つ。同じ画像が続く区間を 1 ドローコールにまとめる。
- `lod` は CPU で計算してインスタンスに持たせる（`log2(テクセル / 物理 px)` の切り捨て）。四捨五入（ハードウェアの最近傍ミップ）では Chrome より明らかにぼやけた。レイヤーの中でも軸に平行なので、微分に頼る必要がない。
- 不透明な画像で、角丸も透明度もないときはブレンドなしのパイプラインで描く（[11](11-gpu-fill.md)）。
- テクスチャは `Weak<Bitmap>` を持ち、参照が消えたら次のフレームで解放する。解放の判断はコア側のキャッシュが持つ。
- 繰り返しはサンプラーの `Repeat` で描く。1 px の模様を 4K 全面に敷いても 1 インスタンスで済む。
- 差分: `ImageDraw` の範囲は `rect`（繰り返しなら `shape`）∩ `shape` ∩ `clip`。比較は `Arc` のポインタと `frame` で行う。

## アニメーション（GIF、APNG、WebP）

- 全フレームをデコードしてから再生を始める。フレームは表示の大きさまで縮小し、ミップは作らない。
- テクスチャは 1 枚で、フレームが変わったときだけ `write_texture` する。GPU のメモリは 1 フレーム分で済む。
- タイマーだけで進める（120 Hz で回らない）。何もしていないときの CPU は 0 のまま。
- 表示範囲に入っているノードがなければ止める。
- 遅延が 10 ms 以下のフレームは 100 ms として扱う（ブラウザと同じ）。
- 変わるのは画像の矩形だけなので、ダメージもその分だけになる。

## 形式と依存

| 形式 | クレート | 備考 |
|---|---|---|
| PNG / APNG、JPEG、GIF、WebP | `image`（`png`, `zune-jpeg`, `gif`, `image-webp`） | Pure Rust。ネイティブ依存なし |
| 色変換 | `moxcms` | `image` がすでに依存している |
| SVG | `resvg`（`anti-webui` の機能 `svg`、既定で有効） | |
| SVG のテキスト | `resvg` の `text`（`anti-webui` の機能 `svg-text`、既定で有効） | 本文に `<text` があるときだけシステムフォントを読む |

ストリップ後のバイナリサイズの増分（実測、単体のプローブで）:

| 構成 | 増分 |
|---|---|
| PNG + JPEG + GIF + WebP | 約 750 KiB |
| 上記 + SVG（テキストなし） | 約 2.3 MiB |
| 上記 + SVG（テキストあり） | 約 3.9 MiB |

AVIF は `image` の中ではネイティブの dav1d が必要なので入れない。`http://` の読み込みはしない（ネットワークはフレームワークの範囲外）。バイト列を渡せば表示できる。

## 既定のコンポーネント

`anti-webui-widgets`。

| 関数 | 中身 |
|---|---|
| `image_view(src, alt)` | `Img`。読み込み中はテーマの色で埋め、届いたらふわっと出し、失敗したら `alt` を表示する |
| `avatar(name)` | 頭文字の丸 |
| `image_avatar(src, name)` | 画像の丸。読み込み中と失敗時は頭文字 |
| `media_card(src, alt)` | 上端まで画像が届くカード。下に続くものは `card_body()` に入れる |

どれも `Img` / `Div` を返すので、`.w()`、`.rounded()`、`.object_fit()` などをそのまま続けられる。

## MD3E

`anti-webui-md3e`。

| 関数 | 中身 |
|---|---|
| `image(src)` | `surface_container_highest` を読み込み中の色にして、300 ms でフェードインする。失敗時は `broken_image` のアイコン |
| `avatar(src, name)` | 40 dp の丸。リストの `leading` に入れる大きさ。読み込み中と失敗時は頭文字 |
| `initials_avatar(name)` | 頭文字の丸（`primary_container`） |
| `card_media(src)` | カードの上端に置く画像 |
| `carousel_image(src)` | カルーセルの項目。全体に広がるように切り抜く |

カルーセルの項目は Compose の `maskClip` と同じく、切り抜きの中央に寄せる（パララックス）。これで細くなった項目でも画像の真ん中が見える。

## 検証

### Chrome との比較

同じページを Chrome（`--headless=new --enable-gpu-rasterization --use-angle=gl-egl`、GPU ラスタ）と AntiWebUI で描いて、50 ケースを画素で比べた。差は RGB のうち最大のチャンネルの差（0〜255）の平均。

| スケール | 全ケースの平均 | 平均が 0.5 未満のケース |
|---|---|---|
| 1.0 | 0.31 | 46 / 50 |
| 1.8 | 0.14 | 49 / 50 |

対象: `object-fit` の全種類、`object-position`、角丸、枠線とパディング、`opacity`、縮小（1.4〜13 倍）、拡大、`pixelated`、透過、JPEG / WebP / GIF、EXIF の向き（3, 6, 8）、SVG（等倍、2 倍、比の違う箱、`preserveAspectRatio="none"`）、`aspect-ratio`、`min-*` / `max-*`、`background-size` / `position` / `repeat` の組み合わせ。

差が 0.5 以上のケース:

| ケース | スケール | 平均 | 中身 |
|---|---|---|---|
| `gif` | 1.0 | 7.20 | Chrome が撮影時に次のフレームへ進んでいた。ソフトウェアラスタの撮影とは差 0.00 |
| `bg-contain-r` | 1.0 | 1.87 | GPU ラスタの Chrome だけが違う。ソフトウェアラスタの Chrome とは差 0.14 |
| `down-8.3x` | 1.0 | 0.67 | 細い線の縮小結果の違い |
| `jpeg` | 1.0 | 0.57 | デコーダの違い（最大の差 6） |
| `border-pad` | 1.8 | 1.25 | 枠線とパディングを整数 px に丸めるレイアウトの方針の違い |

実測で分かって合わせたこと:

- 縮小は「箱以上で最小のレベル」を読む。ハードウェアの最近傍ミップ（四捨五入）だと差が 1.3 → 0.5 に悪化した。
- 繰り返す背景は端数のタイルのまま繰り返す。`16 px × 1.8 = 28.8` を丸めると、差が 0.88 → 0.02 に悪化した。
- SVG は比の違う箱の中央に比を保って描く。

大きさの比較（`getBoundingClientRect`）も一致した: 列で伸びる（300×200）、行で伸びる（240×160）、縮まない（120×80）、`min-width: 0` で縮む（60×40）、`flex: 1`（400×266.66、こちらは 400×267）、`max-width: 100%`（60×40）、`aspect-ratio`（200×100、中身が大きいと 200×90）、枠線とパディング込み（130×96.66、こちらは 130×97）。

違うところ: 読めない画像は Chrome では壊れたアイコンの分の大きさになるが、こちらは 0×0 で何も描かない。`image_view` などは `alt` やアイコンを出す。

### 効率

`bench_images`（RTX 5070、20 コア、scale 1.8、2340×1616、リリースビルド。2 回の範囲）:

| 内容 | 画面に出るまで | UI スレッドのフレーム（p50 / 最大） | GPU メモリ | 全画素を持った場合 |
|---|---|---|---|---|
| 12 MP の JPEG 1 枚を 300×225 で | 77〜88 ms | 0.08〜0.18 / 0.71〜0.77 ms | 3.8 MiB | 61 MiB |
| 12 MP の JPEG 8 枚を 150×112 で | 168〜185 ms | 0.15〜0.23 / 0.67〜0.71 ms | 7.6 MiB | 488 MiB |
| 12 MP の JPEG 1 枚を 1000×750 で | 70〜74 ms | 0.10〜0.16 / 2.56〜2.66 ms | 15.3 MiB | 61 MiB |
| 12 MP の PNG 1 枚を 300×225 で | 88〜89 ms | 0.08〜0.09 / 0.73〜0.76 ms | 3.8 MiB | 61 MiB |
| 12 MP の PNG 4 枚を 150×112 で | 100〜122 ms | 0.17〜0.23 / 0.88〜0.90 ms | 3.8 MiB | 244 MiB |

小さな画像 2000 枚（表示は 40×40）のスクロール: p50 0.09 ms、p99 0.4〜0.8 ms、最大 2.8 ms（600 ステップ）。GPU にあるのは見える範囲の周辺の 222 枚（4.6 MiB）で、残りはデコードしていない。

実ウィンドウ（RTX 5070、ShojiWM、120 Hz）:

- 画像のページで何もしていない 3 秒の CPU は 2 tick。
- 96×96 の GIF（25 fps）を再生中、ダメージは毎フレーム 1.7%（画像の矩形のみ）。3 秒の CPU は 8 tick。

**未計測:** Chrome とのメモリの比較、内蔵 GPU、60 Hz。

### 回帰テスト

- `anti-webui-core/tests/images.rs`: 大きさ、通知、切り替え、デコードの大きさ、カリング、予算、ダメージ、アニメーション。
- `anti-webui-app/tests/images.rs`: 実際の画素。Vulkan ローダーが複数スレッドからのインスタンス生成でまれに落ちるので、このファイルは GPU コンテキストを 1 つずつ使う。
- `anti-webui-widgets/tests/images.rs`、`anti-webui-md3e/tests/images.rs`: 部品の状態遷移。

## やっていないこと

- 手での確認。実ウィンドウでの表示とアニメーションは確かめたが、スクロール中の見え方、ウィンドウのリサイズ、ポインタ操作は合成イベントだけで確かめた。
- `srcset` の候補からの自動選択、`<picture>`。`window_scale()` を読んで `img` のクロージャで切り替えられる。
- `mask-image`、`currentColor` の SVG、グラデーション（`fill` のシェーダーで書く）、複数の `background-image`、`background-clip`、`background-origin`（パディングボックス固定）、`background-attachment`。
- `ImageSource` を `Image`（`filter` の式）の入力にすること。
- AVIF、HEIC、JPEG XL、ネットワークからの読み込み、ディスクキャッシュ。
- 長いアニメーションを少しずつデコードすること。全フレームの合計に 256 MiB の上限があり、超えた分は切り捨てて、そこまでを繰り返す。
- JPEG を DCT の段階で縮小すること。
