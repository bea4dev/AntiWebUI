# 24. 動画

`<video>` にあたるものを作る。CSS でできることは同じ書き方で同じ結果にする。効率は Chromium に合わせる。

```rust
// <video src="clip.mp4" autoplay loop muted style="width: 640px; object-fit: cover; border-radius: 12px">
let player = video_player();
video("clip.mp4")
    .player(player)
    .autoplay(true)
    .looped(true)
    .muted(true)
    .w(640.0)
    .object_fit(ObjectFit::Cover)
    .rounded(12.0)
    .on_error(|e| match e {
        VideoError::Unavailable(_) => { /* FFmpeg が無い */ }
        VideoError::UnsupportedCodec(_) => { /* デコーダが無い */ }
        _ => {}
    });

button("再生").on_click(move |_| player.play());
text(move || format!("{:.0} / {:.0}", player.current_time(), player.duration()));
```

## ライセンス

**本体は `MIT OR Apache-2.0` のまま、FFmpeg のコードを含めない。** FFmpeg は実行時に `dlopen` で読むだけにする。

| 立場 | 扱い |
|---|---|
| AntiWebUI の配布物 | FFmpeg を含まない。ビルドにも FFmpeg は要らない |
| FFmpeg の ABI の記述 | `anti-webui-video` に手書きする。関数の宣言、構造体のオフセット、定数だけで、ヘッダの写しは持たない（LGPL 2.1 第 5 条の「数値、データ構造の配置、アクセサ」にあたる） |
| アプリの作者 | 既定ではシステムの FFmpeg を使うので同梱は不要。同梱するなら LGPL の義務（ソースの提供など）はアプリ側が負う |
| 特許（H.264 / HEVC など） | どの方式でも残る。AntiWebUI はデコーダを配らないが、アプリの配布や利用の判断はアプリ側で行う |

Arch の FFmpeg は GPLv3 のビルドである（`avcodec_license()` が `GPL version 3 or later` を返す）。利用者の PC でそれを読み込むこと自体は GPL の配布の条件に触れないが、**クローズドソースのアプリが GPL ビルドの読み込みを前提に配布してよいかは解釈が分かれる**。そこで、次のどちらかを選べるようにする。

```rust
update_settings(|s| s.video.libraries = VideoLibraries::LgplOnly);
```

`LgplOnly` では `avcodec_license()` が LGPL を返すものしか使わない。Arch では、LGPL の `.so` を同梱しない限り `VideoError::Unavailable` になる。

法律の専門家の確認は受けていない。製品として配布する前に確認すること。

## FFmpeg の読み込み

動画を使わないアプリは何も読み込まない。最初の動画が開かれるときに、動画のスレッドで次の順に探す。

1. `settings.video.library_dir`、または実行ファイルの隣（`<exe>/ffmpeg/`）にある `.so`
2. システムの FFmpeg（`VideoLibraries::LgplOnly` でも読むが、LGPL でなければ使わない）
3. どちらも無ければ `VideoError::Unavailable`

普通の動的リンクだと、無いときに `main` の前でプロセスが起動に失敗する。実行時に読み込むのでそれが起きない。

FFmpeg の ABI はメジャーバージョンごとに変わる。バージョンごとのオフセットの表を持ち、表にない組み合わせは使わない。

| FFmpeg | avcodec | avformat | avutil | swresample | swscale |
|---|---|---|---|---|---|
| 9.x | 63 | 63 | 61 | 7 | 10 |

表に行を足すのは、実際に再生して確かめてからにする。数字が 1 つ違っていても失敗にならず、クラッシュするからである。

オフセットの表は `src/anti-webui-video/tools/offsets.c` を各バージョンのヘッダでコンパイルして得た数値である。起動時に `avcodec_version()` などの実行時の値で行を選ぶ。

ピクセル形式の値は `av_get_pix_fmt("vaapi")` のように実行時に名前で引く。

## 構成

```
anti-webui-core            video() 要素、VideoPlayer、VideoBackend トレイト、VideoFrame、Primitive::Video
                           （FFmpeg を知らない。バックエンドが無ければ Unavailable）
anti-webui-video           VideoBackend の実装。FFmpeg と ALSA を dlopen、分離、デコード、音声、時刻合わせ
                           （機能 video で有効。結線は anti-webui-app）
anti-webui-wgpu            VideoFrame の描画。メモリの画素の転送と dmabuf の取り込み
```

`anti-webui` の機能 `video` は既定で無効である。無効でも `video()` は使えて、`VideoError::Unavailable` になる。アプリ側に `cfg` を書かなくて済む。

開くのは最初のフレームの直前まで待つ。ツリーは窓より先にできるが、デコーダは描画に使う GPU を知る必要があるからである。

## 流れ

```
分離スレッド ──→ 映像の packet ─→ 映像スレッド ─→ フレーム（4 枚まで） ─→ UI スレッドが時刻で選ぶ ─→ 描画
           └──→ 音声の packet ─→ 音声スレッド ─→ ALSA
                                                 └─ 再生位置が全体の時計
```

スレッドは 1 本の動画につき 3 本で、アプリの作者からは見えない（[UI スレッドは 1 本](05-rendering.md)の方針）。

| | Chromium | AntiWebUI |
|---|---|---|
| デコード済みの先読み | 4 枚（`limits::kMaxVideoFrames`） | 同じ |
| フレームの選択 | コンポジタのスレッド。UI が重くても止まらない | UI スレッド。UI が 1 リフレッシュを超えて止まるとカクつく（音声は止まらない）。表示スレッドの分離は [10](10-frame-pacing.md) の将来の課題 |
| 時計 | 音声。無ければ壁時計 | 同じ |
| 割り振り（24 fps → 60 Hz） | `VideoCadenceEstimator` | 同じ考え方の簡略版（下記） |
| 画面に出ない間 | 映像を捨てて音声は続ける | 同じ |
| デコードのスレッド数 | コーデックと解像度で決める | 4 まで（下の実測のとおり、メモリがスレッド数に比例する） |

### 時刻合わせ

- 音声がある間の時計は「書き込んだ分の終わりの時刻 − `snd_pcm_delay`」。無音（`muted`）でも無音を書き続けるので、時計は同じである。
- 音声が無い、出力先が無いときは壁時計を使う。
- UI のフレームごとに、そのフレームが画面に出るころの時刻に合うものを選ぶ。

### 割り振り

フレームの長さとリフレッシュの周期の比が整数に近い（30 fps → 60 Hz は 2、24 fps → 120 Hz は 5）と、時計のわずかな揺れで同じフレームが 1 回や 3 回になる。

- 比が整数に近いときは「N リフレッシュごとに 1 枚」に固定する。ずれが半フレームを超えたら直す。
- 整数に近くない（24 fps → 60 Hz）ときは時刻どおりに選ぶ。これで 3:2 になる。
- リフレッシュの周期が分からない、VRR のときは時刻どおりに選ぶ。

純粋な関数にして、揺れのある vsync の模擬で確かめる（`anti-webui-video/src/cadence.rs`）。

リフレッシュの数え方と、フレームを作る時刻は実機で 3 回変えた。120 Hz の出力で、理想の間隔から外れたフレームの割合は次のとおり。

| 方法 | 24 fps | 30 fps | 60 fps |
|---|---|---|---|
| 経過時間を周期で割る。vsync の直後（+1 ms）に作る | 7% | 3% | 2% |
| 基準をフレームコールバックの受信時刻にする | 42% | 38% | 46% |
| 基準を `wp_presentation` の表示時刻にする。vsync の直後に作る | 8% | 2% | 3% |
| 基準は同じ。**リフレッシュの中間で作る（採用）** | 3% | 0.4% | 1% |

- フレームコールバックの受信時刻は揺れるので、基準にしてはいけない。
- vsync の直後に作ると、GPU の完了（1.5〜3.5 ms 後）がコンポジタの取り込みに間に合うかどうかで、表示が 1 リフレッシュずれる。遅れた回は起こす時刻は正確で（1.09 ms 後）、commit から表示までだけが 6.9 ms から 15 ms に伸びていた。
- そこで、表示したいリフレッシュの 2 つ前の中間に作る。音声との差は最大で約 1 リフレッシュ増える。

### 何もしないときの負荷

変わるのは動画の矩形だけで、レイアウトもスタイルの解決も走らない。24 fps なら、120 Hz の出力でも描くのは毎秒 24 回である。止めている間は何も起こさない。

## 描画

```rust
Primitive::Video(VideoDraw {
    frame: Arc<VideoFrame>,
    rect: RectI,   // object-fit を解決した矩形
    shape: Clip,   // 自分の形。コンテンツボックスの内側の角丸
    clip: Clip,    // 祖先のクリップ
    opacity: f32,
})
```

角丸、`opacity`、`filter`、`transform`、影、枠線は他の要素と同じように効く。パイプラインは最初の動画で作るので、動画を使わないアプリの起動は遅くならない。

### 3 段階

| 段階 | 方法 | 状態 |
|---|---|---|
| 1 | メモリの画素を `R8` / `RG8` のテクスチャへ転送し、シェーダーで RGB に変換する。どの環境でも動く | 実装 |
| 2 | VA-API の画像を dmabuf として Vulkan に取り込む（コピーなし） | 実装 |
| 3 | サブサーフェスでコンポジタに直接渡す | 見送り（下記） |

ハードウェアデコードが使えない（VA-API が無い、デバイスが違う）、取り込めないときは段階 1 に戻る。動画のストリームごとに判断するので、アプリから見て違いはない。

### 段階 2 の取り込み

wgpu 30 の `EXTERNAL_TEXTURE` は DX12 と Metal だけで、Vulkan では使えない。自分で取り込む。

1. `av_hwframe_map` で VA の画像を `AV_PIX_FMT_DRM_PRIME` にし、レイヤー（輝度、色）ごとの fd、offset、pitch、modifier を得る。
2. レイヤーごとに `R8` / `RG8`（10 bit は `R16` / `RG16`）の `VkImage` を作る（`VK_EXT_image_drm_format_modifier` の explicit layout）。同じ VA の画像は `fstat` の inode で見分けて使い回す。
3. `QUEUE_FAMILY_FOREIGN_EXT` から `SHADER_READ_ONLY_OPTIMAL` へのバリアを自分で発行し、`initial_state: RESOURCE` で wgpu に渡す。wgpu に `UNDEFINED` から遷移させると、内容を捨ててよいことになってしまう。
4. フレームが GPU で読み終わるまで `Arc<VideoFrame>` を持ち続ける。読んでいる間は VA の画像がデコーダに再利用されない。

デコーダのプールの枚数は固定なので、`extra_hw_frames` で先読みの 4 枚と描画中の分を足す。

`av_hwframe_map` の中で `vaSyncSurface` が呼ばれるので、取り込む時点でデコードは終わっている。

GPU は表示に使っているものと同じデバイス（`DmabufContext::render_node`）で開く。

### 段階 3 を見送る理由

サブサーフェスに直接渡せば、コンポジタがハードウェアのプレーンに載せられて電力が減る可能性がある。ただし、

- `wl_subsurface.set_position` と `wp_viewport.set_destination` は論理ピクセルの整数しか取れない。スケール 1.8 では物理ピクセルに揃わず、枠線や周りの UI とずれる（[01](01-pixel-model.md) の方針に反する）。全画面なら問題ない。
- ShojiWM には `wp_color_representation_v1` が無く、YUV の変換式と範囲を指定できない。
- 角丸、半透明、上に重なる UI とは両立しない。

全画面再生に限って後から足せる。電力の実測で差が出たら検討する。

### 色

| 項目 | 扱い |
|---|---|
| 行列 | フレームの `colorspace`。未指定なら高さ 720 未満で BT.601、それ以上で BT.709（Chromium と同じ） |
| 範囲 | `color_range`。未指定なら limited |
| HDR（PQ、HLG） | 10 bit のまま読み、シェーダーで SDR にトーンマップする |
| 向き | スマートフォンの動画の回転（display matrix）を適用する。`natural` の幅と高さも入れ替わる |

標準の出力先は sRGB として扱い、BT.709 の伝達関数の違いは無視する。

### 縮小

ミップは作らない（フレームごとに作ると重い）。4K を小さな箱に出すと線形補間だけではチラつくので、2 倍を超える縮小では 4 点で読む。

## API

| API | HTML | 備考 |
|---|---|---|
| `video(src)` | `<video src>` | `src` はパス、URL（FFmpeg が読めるもの）、またはそれを返すクロージャ。変わると開き直す |
| `.player(player)` | | 操作と状態の入口 |
| `.autoplay(b)` | `autoplay` | 既定は無効。無効のときは最初のフレームで止まる |
| `.looped(b)` | `loop` | |
| `.muted(b)` | `muted` | |
| `.object_fit(f)`、`.object_position(x, y)` | `object-fit` など | `img` と同じ |
| `.aspect_ratio(r)` | `aspect-ratio` | |
| `.on_load(f)` | `loadedmetadata` | `f(width, height, duration)`。幅と高さは論理 px |
| `.on_ended(f)` | `ended` | |
| `.on_error(f)` | `error` | `f(&VideoError)` |
| `.child(view)` | | 動画の上に重ねる。操作のバーなどに使う |

大きさは `img` と同じ置換要素の規則で決まる。実寸が分かるまでは 0×0 になる。

| `VideoPlayer` | HTML | 備考 |
|---|---|---|
| `video_player()` | | `Copy`。シグナルでできている |
| `.play()`、`.pause()` | `play()`、`pause()` | |
| `.seek(sec)` | `currentTime =` | 直前のキーフレームから読み、その時刻まで表示を進めない |
| `.set_volume(v)`、`.set_muted(b)` | `volume`、`muted` | |
| `.current_time()`、`.duration()` | | 読むと購読される。`current_time` は 4 Hz で更新（`timeupdate` と同じ頻度） |
| `.paused()`、`.ended()`、`.volume()`、`.muted()` | | |

| 関数 | HTML | 備考 |
|---|---|---|
| `video_availability()` | | FFmpeg を読み込めるか。理由とバージョンも返す |
| `can_play_video(mime)` | `canPlayType()` | `"video/mp4; codecs=avc1"` などに答える |

| `VideoError` | HTML の `MediaError` | 意味 |
|---|---|---|
| `Unavailable` | | FFmpeg が無い、対応外のバージョン、`LgplOnly` で LGPL でない、機能 `video` が無効 |
| `UnsupportedCodec` | `MEDIA_ERR_SRC_NOT_SUPPORTED` | FFmpeg はあるがデコーダが無い |
| `Source` | `MEDIA_ERR_NETWORK` | 開けない、読めない |
| `Decode` | `MEDIA_ERR_DECODE` | 壊れている |

## 音声

ALSA（`libasound.so.2`）を `dlopen` で読む。PipeWire の環境でも `pipewire-alsa` を通して動く。Rust の音声クレートは `libasound` にリンクするので、無い環境で起動できなくなるため使わない。

- 形式は `S16`、2 ch、元のサンプルレートで、`libswresample` で変換する。
- 音量はこちらで掛ける。
- 出力先が開けなければ音声なしで再生する（エラーにはしない）。`ANTIWEBUI_AUDIO_DEVICE` で出力先を指定できる（`none` で出さない）。

## 対象外

- HLS / DASH、DRM（Widevine など）、`<track>`、MSE。
- HTTP(S) は FFmpeg が読める範囲だけ（プログレッシブ再生）。バッファリングの表示や再接続の調整はしない。
- 標準の操作のバー（`<video controls>`）は、まだ `.child` で自分で作る（`src/anti-webui/examples/video.rs`）。

## 同梱するとき

`src/anti-webui-video/tools/build-ffmpeg-lgpl.sh` が、再生に必要なものだけの LGPL の `.so` を作る。できたものを `<exe>/ffmpeg/` に置く。別のバージョンを同梱するなら、`tools/offsets.c` で数字を出して `LAYOUTS` に足し、再生して確かめる。

## 未確認

- **LGPL の FFmpeg とこのスクリプト。** 環境の制限で、ダウンロードしたソースのビルドを実行できなかった。同梱の探索はシステムのライブラリのコピーで確かめた。LGPL の判定は文字列の単体テストと、GPL ビルドが断られることまで。
- **FFmpeg 8 以前。** 表に無いので `Unavailable` になる。多くのディストリビューションはまだこちらなので、早めに足したい。
- **実際の動画。** すべて合成映像で測った。
- **音を耳で聞くこと。** サンプルの中身はファイルに書く ALSA のデバイスで、再生時間は実機のデバイスで確かめた。映像と音のずれは測っていない。
- 60 Hz の出力と、3:2 の見た目（模擬のテストのみ）。
- NVIDIA。VA-API が無いので CPU デコードになるはず。後回しにしている。再開するときは **Vulkan Video（FFmpeg 経由）から実装する**（作者の希望）。追加の導入なしでゼロコピーを狙える唯一の方式である。`nvidia-vaapi-driver` と NVDEC（CUDA）でコピーする経路は、必要になったら検討する。
- HTTP(S) の再生。
- HDR の実機の見た目。トーンマップは PQ / HLG の式どおりだが、本物の HDR 動画で見ていない。
- Windows / macOS。FFmpeg と ABI の扱いは同じ形で使えるはずだが、音声と取り込みは別に作る。

## 実測

Radeon 880M、スケール 1.8、120 Hz、窓は 1728×1616 px。合成映像（1080p60 H.264 8 Mbps、4K60 H.264 40 Mbps）を `examples/video` で全体を再生した。CPU は 1 コア = 100%。Chromium はプロセス全部の合計で、`--enable-features=AcceleratedVideoDecodeLinuxGL,VaapiVideoDecoder`。メモリは PSS と、`fdinfo` の VRAM + GTT。

| | CPU | PSS | GPU メモリ |
|---|---|---|---|
| AntiWebUI 1080p ゼロコピー | 6% | 84 MB | 289 + 25 MB |
| AntiWebUI 1080p CPU デコード | 31% | 127 MB | 227 + 12 MB |
| Chromium 1080p | 21% | 431 MB | 165 + 22 MB |
| AntiWebUI 4K ゼロコピー | 4% | 128 MB | 359 + 24 MB |
| AntiWebUI 4K CPU デコード | 49% | 282 MB | 227 + 12 MB |
| Chromium 4K | 17% | 480 MB | 381 + 22 MB |

- 1080p の GPU メモリは Chromium より多い。差のほとんどは動画ではなく窓の側（CPU デコードでも 227 MB）で、理由は未調査。
- Chromium の窓の大きさは同じになるよう指定しただけで、確かめていない。

| 指標 | 値 |
|---|---|
| フレームの UI の処理（p50 / p99） | 0.01 / 0.05 ms |
| GPU（p50 / p99） | 0.52 / 1.25 ms（1080p）、0.63 / 1.49 ms（4K） |
| 再描画の範囲 | 動画の矩形のみ（窓の 60%） |
| フレームの間隔 | 60 fps → 120 Hz で 98% 以上が 2 リフレッシュ |
| 機能 `video` の増分 | 138 KiB（ストリップ後、`widgets`） |
| 依存するライブラリの増加 | 0（`ldd` に FFmpeg も ALSA も出ない） |

FFmpeg を隠して起動しても、窓は普通に出て、FFmpeg は読み込まれない（`bwrap` で確認）。

実装の前に、合成映像を `ffmpeg` で 10 秒再生して測った値（CPU は 1 コア = 100%）。実際の動画ではソフトウェアデコードはもっと重くなる。

| 動画 | CPU（4 スレッド） | CPU（VA-API） | メモリ増（4 スレッド） | メモリ増（全 20 スレッド） | GPU メモリ（VA-API） |
|---|---|---|---|---|---|
| 1080p60 H.264 | 28% | 6% | 56 MB | 127 MB | 106 MB |
| 4K60 H.264 | 44% | 6% | 148 MB | 431 MB | 260 MB |
| 1080p60 AV1 | 22% | 4% | 59 MB | 100 MB | 100 MB |
| 4K60 AV1 | 82% | 4% | 231 MB | 404 MB | 276 MB |
