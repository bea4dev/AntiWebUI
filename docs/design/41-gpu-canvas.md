# 41. GPU キャンバス（`gpu_canvas`）

利用者が wgpu で直接描き、それを UI の箱にはめ込む。Web の `<canvas>` に WebGPU で描くのに当たる。3D の土台で、UI 要素の 3D 変形（CSS の `rotateX` と `perspective`）と、宣言的な 3D シーン（three.js に当たるもの）は、必要になったらこの上に足す。

## API

```rust
use anti_webui::prelude::*;
use anti_webui::gpu::{GpuFrame, GpuInit, GpuPaint, wgpu};

struct Spinner {
    pipeline: wgpu::RenderPipeline,
    uniforms: wgpu::Buffer,
    bind: wgpu::BindGroup,
}

impl GpuPaint for Spinner {
    /// 描くのに要る値。前回と等しければ描き直さない
    type Props = f32;

    /// UI と同じ device で 1 回。GPU が変わったら作り直す
    fn new(gpu: &GpuInit) -> Self {
        // gpu.device、gpu.queue、gpu.format（または gpu.srgb_format）でパイプラインを作る
    }

    /// Props が変わったとき、箱の大きさが変わったときだけ呼ばれる
    fn render(&mut self, angle: &f32, frame: &mut GpuFrame) {
        frame.queue.write_buffer(&self.uniforms, 0, bytemuck::bytes_of(angle));
        let mut pass = frame.begin_pass(Some(wgpu::Color::TRANSPARENT));
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind, &[]);
        pass.draw(0..3, 0..1);
    }
}

let angle = signal(0.0f32);
gpu_canvas::<Spinner>(move || angle.get())
    .w(320.0).h(200.0)
    .rounded(16.0)
    .on_pointer_move(move |e| angle.set(e.local.x * 0.01))
    .child(text("HUD"))
```

- `gpu_canvas::<T>(props)` の `props` は読んだシグナルが変わるたびに走る（`fill` と同じ）。返した値が前回と等しければ何もしない。止まっていれば 1 フレームも作らない。ずっと動かすときは `request_frame` で時刻のシグナルを進める。
- 箱は置換要素で、HTML の `<canvas>` と同じく既定の大きさは 300×150（論理 px）。片方だけ決めると 2:1 を保つ。
- 描き先は箱のコンテンツボックスの物理ピクセルそのもの。`canvas.width = clientWidth * devicePixelRatio` を自分で書く必要はない。
- 角丸、`opacity`、`transform`、`filter`、影、枠線、`overlay` は他の要素と同じように効く。子は上に重なる（`video` と同じ）。
- 当たり判定とイベントは普通の要素と同じ。`e.local` は箱の中の論理 px。
- 2:1 を保つので、`flex_1()` と `h(pct(100.0))` だけでは幅が高さの 2 倍に決まり、縮まない（CSS の `<canvas>` と同じ）。広がった箱に合わせるときは `min_w(0.0)` を足すか、`w` と `h` を両方決める。

| 型 | 中身 |
|---|---|
| `GpuInit` | `device`、`queue`、`format`（`Rgba8Unorm`）、`srgb_format`（`Rgba8UnormSrgb`、GL では `None`） |
| `GpuFrame` | `device`、`queue`、`encoder`、`target`（`format` のビュー）、`target_srgb`、`size`（物理 px）、`scale`（論理 1 px が何ピクセルか） |
| `GpuFrame::begin_pass(clear)` | `target` に色だけのパスを開く。`None` なら前の内容を残す |
| `GpuFrame::begin_srgb_pass(clear)` | `target_srgb` に開く。リニアで書けば保存時に sRGB へ変換される |

`wgpu` は `anti_webui::gpu::wgpu` から使う。AntiWebUI はフォークした wgpu に載っていて、crates.io の wgpu とは型が合わない。wgpu のメジャーバージョンを上げることは AntiWebUI の破壊的変更になる。

## 方式

```
props が変わる ─▶ version を進める ─▶ 箱の chunk だけが変わる ─▶ 変更矩形はその箱だけ
                                                       │
renderer: 描くフレームで、変更矩形に掛かる canvas を集める
   ├─ version か大きさが変わっていれば、利用者の render を呼ぶ（専用の encoder、先に submit）
   └─ テクスチャを画像と同じパイプラインで合成（角丸、クリップ、opacity）
```

### core と renderer の分担

core は wgpu を知らない（[05](05-rendering.md)）。core の `NodeKind::Canvas` が持つのは次だけ。

- `id`: renderer 側で描き手を探すための番号。
- `version`: `props` が変わるたびに 1 進む。`Primitive::Canvas` に入るので、差分は箱だけを汚す。
- `keep`: 描き手を生かしておく `Rc<dyn Any>`。ノードが消えると描き手も消える。

描き手（`T` と最新の `Props`）は `anti-webui-wgpu` のスレッドローカルの表に `Weak` で置く。描画はすべて UI スレッドなので（[10](10-frame-pacing.md) の「スレッド」）、ロックは要らず、`T` に `Send` も要らない。`spawn_window` の窓は自分のスレッドで作って自分のスレッドで描くので、表もそのスレッドのものになる。

`Scene` に `Rc` を入れないのは、`Scene` が `Arc` で持たれるため。

### 変更は箱の中だけ

`props` が変わったときは `mark_shader_of` と同じく、その箱を含むレイヤーだけが描画リストを作り直す（[14](14-shaders.md) の「ペイントの省略」）。`Primitive::Canvas` の比較は `version`、位置、形、クリップ、`opacity` なので、変更矩形は箱だけになる。

### 描くタイミング

- renderer がフレームを準備するとき、変更矩形に掛かる canvas を集める。掛からないものは描かない（テクスチャの中身がそのまま使える）。
- テクスチャの大きさは `content box × レイヤーの倍率 k`（[12](12-transform.md)）。`scale(2.0)` で拡大したときも、ぼけずに描き直される。`max_texture_dimension_2d` を超えるときは縮めて描き、合成で引き伸ばす。
- 描き直すのは `version` か大きさか device が変わったときだけ。

### 利用者のコードのエラーから UI を守る

- 利用者には **専用の encoder** を渡し、`finish` してすぐ `submit` する。同じ queue なので、UI の描画（後で submit）より先に実行され、合成はその結果を読む。UI の encoder に書かせると、利用者のパスが 1 つ不正なだけで UI のフレーム全体が捨てられる。
- `new` と `render` は `ErrorFilter::Validation` の error scope で囲む。エラーが出たら、標準エラーに型の名前を付けて 1 度だけ出し、その箱は何も描かない（シェーダーのコンパイルエラーと同じ）。パニックはしない。
  - `render` のエラーは、次に `props` か大きさが変わったときにもう一度試す。
  - `new` のエラーは、device が変わるまで試さない。
- submit が 1 回増えるのは描き直すフレームだけ。

### device が変わったとき

今は、窓の device は最初のフレームで作られ、窓が閉じるまで変わらない（device lost から作り直す仕組みは無い）。それでも描き手は自分を作った `wgpu::Device` を覚えていて（`Device` は同一性で比べられる）、違う device で描くことになったら `new` から作り直す。これが効くのは今のところ、`ANTIWEBUI_DUMP_FRAME` のように別の renderer が描くときだけだが、作り直す仕組みが入っても利用者のコードは変わらない。`Props` は残るので、見た目も変わらない。

### 色とアルファ

- UI はガンマ空間のまま非 sRGB のテクスチャで合成している（[05](05-rendering.md)）。描き先は `Rgba8Unorm` で、そこに書いた値がそのまま UI と同じ色になる。
- 3D のライティングはリニアで計算したいので、同じテクスチャの `Rgba8UnormSrgb` のビューも渡す（`view_formats`）。GL は `VIEW_FORMATS` が無いので `None`。
- 出力はプリマルチプライドアルファ。透明に消せば、後ろの UI が透けて見える。
- テクスチャは前回描いた内容を持ち続ける。消すかどうかは利用者が `begin_pass` で決める。

### 合成

画像（[22](22-images.md)）と同じ `picture` パイプラインで、コンテンツボックスの内側の角丸に切り抜いて描く。追加のシェーダーは無い。

### メモリ

テクスチャは箱 1 つにつき 1 枚（物理 px × 4 バイト。1000×600 で 2.4 MB）。ノードが消えると、次のフレームで捨てる。`content-visibility` で飛ばされている間も持ち続ける。

## `gpu_info`: 何が描いているか

GL に落ちたか、ソフトウェアで描いているかを、アプリが知るための口。「GL なら重い効果を切る」「設定画面に使っている GPU を出す」に使う。これまでは `ANTIWEBUI_DEBUG_DAMAGE=1` のときに標準エラーへ出すだけだった。

```rust
let info = gpu_info();      // Option<GpuInfo>。最初のフレームの前は None。読むと購読される
let fallback = info.is_some_and(|g| g.backend == GpuBackend::Gl || g.kind == GpuKind::Cpu);
text(move || gpu_info().map(|g| g.to_string()).unwrap_or_default())
// "AMD Radeon 880M Graphics (RADV STRIX1), Vulkan, dmabuf, explicit sync"
```

| `GpuInfo` | 中身 |
|---|---|
| `backend` | `GpuBackend::{Vulkan, Metal, Dx12, Gl, Other}` |
| `adapter` | GPU の名前（ドライバが言うもの） |
| `kind` | `GpuKind::{Integrated, Discrete, Virtual, Cpu, Other}`。`Cpu` は llvmpipe や lavapipe（GPU が無い） |
| `driver` | ドライバの名前と版（`radv Mesa …`）。GL では空のことがある |
| `present` | `GpuPresent::{Dmabuf { explicit_sync }, IoSurface, DxgiSwapchain, Surface}`。`Surface` は wgpu の Surface へのフォールバックで、毎フレーム全面を描く |

- 窓ごとの値。`spawn_window` の窓は別のスレッドで別の device を持ちうるので、そこで読めばその窓のものが返る。
- core は wgpu を知らないので、型は core に持ち、app が presenter を作ったときに wgpu の `AdapterInfo` から変換して `Ui::set_gpu_info` で渡す。値の置き場は `window_active` などと同じ窓ごとの `Info` で、シグナルは最初に読まれたときに作る。
- `gpu_canvas` の中では、`gpu.device.adapter_info()` で wgpu の値を直接見てもよい。
- 構造体と列挙は `#[non_exhaustive]`。項目やバックエンドを足しても破壊的変更にならない。

### 確認

- `anti-webui-core/tests/windows.rs`: 最初のフレームの前は `None`、渡されると購読している効果が 1 回だけ走り、同じ値では走らない。2 つ目の窓に別の値を渡しても 1 つ目の窓の購読は走らず、それぞれの窓で読めばそれぞれの値が返る。
- サンプルの立方体の上に `gpu_info` を出した。この機械では `… (RADV STRIX1), Vulkan, dmabuf, explicit sync`。`ANTIWEBUI_PRESENT=surface WGPU_BACKEND=gl` で起動すると `… (radeonsi, …), OpenGL, wgpu surface` になる。
- GL では sRGB のビューが無いので、サンプルはシェーダーで光を色に直して書く。Vulkan と GL で同じ見た目になる。

## device の limits

これまで device は `downlevel_webgl2_defaults` の limits で作っていた。これでは storage buffer も compute も使えず、3D の多くの書き方ができない。adapter が満たせるときは `Limits::default()`（WebGPU の既定値）で作り、満たせないとき（GL など）だけ WebGL2 の値にする。どちらも `using_resolution` で最大の大きさは adapter に合わせる。

features は今までどおりで、利用者が足す口はまだ無い（必要になったら `App` に足す）。

## 確認

- `anti-webui-app/tests/gpu_canvas.rs`（実際の renderer、GPU が無ければ飛ばす）:
  - コンテンツボックスを埋め、パディングは背景のまま、角丸で切れる。
  - 何も決めなければ 300×150、幅だけなら 2:1。スケール 1.5 では 450×225 の画素に描く。`scale(2.0)` の中では倍の画素に描く。
  - 関係ない変化や、等しい `props` では描き直さない。`props` が変わると 1 回だけ描き、変更矩形は箱だけで、部分描画と全面描画の画素が一致する。
  - `target_srgb` に光の 0.5 を書くと 188 になる。
  - 不正なパス、作れないバッファ: その箱だけ何も描かず、隣の canvas と UI は描かれ、パニックしない。`props` が変わるともう一度試す。
  - ノードが消えるとテクスチャも消える。
- `cargo run --release -p anti-webui --example gpu_canvas`: 4× MSAA、depth、sRGB のビューで光を計算する立方体。ドラッグで回す。ShojiWM（スケール 1.8）で、枠の角丸、上に重ねた文字、透明な背景が正しく出ることを、`ANTIWEBUI_DUMP_FRAME` の画像で見た。
- core、wgpu、app のテスト 877 件が通る。limits の変更の後も、既存の画素のテストは変わらない。

## やっていないこと

- **MSAA と depth の用意**: 利用者が持つ。`frame.size` が変われば作り直す。
- **features を足す口**: 上のとおり。
- **描き先の形式を選ぶ**: `Rgba8Unorm` だけ。HDR（`Rgba16Float`）は UI の合成が 8 ビットなので意味が無い。
- **UI 要素の 3D 変形、宣言的な 3D シーン**: 必要になったら、この上に足す。
