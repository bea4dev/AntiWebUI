# 35. X11 バックエンド

Wayland でできていることを、X11 でも同じ水準でできるようにする。描く仕組み（core、レンダラ、先行描画）はそのまま使い、ウィンドウ、入力、提示、ポップアップ、クリップボード、ドラッグ＆ドロップ、IME を X11 の仕組みにつなぐ。

2026-11-07 に X0 から X4 まで作った。手での確認はまだしていない（「確かめ方」を参照）。

## 方針

- **ツールキットは使わない。** Wayland と同じく、プロトコルを直接話す。`x11rb` を使う。Chromium も自前で生成したプロトコルの層（`ui/gfx/x`）を持ち、Xlib や GTK のウィンドウは使っていない。
- **Wayland と同じ水準にする。** 変わったところだけ描き、変わったところだけをサーバーに伝える。vblank に合わせ、画面に出た時刻を受け取る。明示的同期を使う。このために、wgpu の Surface ではなく、自分で確保したバッファを DRI3 と Present で出す（[06](06-platform-wayland.md) の dmabuf presenter の X11 版）。
- **実行時に選ぶ。** 1 つのバイナリが Wayland と X11 のどちらでも動く。
- **依存を増やしすぎない。**
  - 足すのは `x11rb`（プロトコルは Rust で、通信は libxcb）、`xkbcommon` の `x11` 機能（libxkbcommon-x11）、`xim`（XIM の Rust 実装）。
  - 非同期のランタイムや D-Bus は入れない。
  - Cargo の機能 `x11` で外せるようにする（`wayland` も同じ）。

## バックエンドの選び方

| 条件 | 使うもの |
|---|---|
| `ANTIWEBUI_BACKEND=wayland` または `x11` | その指定 |
| `WAYLAND_DISPLAY` があり、つながる | Wayland |
| `DISPLAY` があり、つながる | X11 |
| どちらもない | エラー |

Chromium も `--ozone-platform-hint=auto` では同じ順で選ぶ。Wayland につながらなければ X11 を試す。

## app の作り替え

今の `anti-webui-app` の `Runner`、`Presenter`、`Pacing` は `anti_webui_wayland::Ctx` を直接呼んでいる。これを、次の形に分ける。

- **受け渡しの型を core に移す。**
  - `SurfaceId`、`BufferId`、`BufferEvent`、`DmabufFeedback`、`DmabufTranche`、`DmabufPlaneDesc`、`SyncPoints`、`TimelineId`、`Presentation`、`Shown` は、中身がデータだけの型である。
  - これらを `anti_webui_core::platform`（doc(hidden)）へ移す。`anti-webui-wayland` は再エクスポートして、今の名前のまま使えるようにする。
- **`app` に `trait Host` を置く。**
  - `Runner`、`Presenter`、`Pacing` が呼んでいるおよそ 45 のメソッドを、そのままトレイトにする。
  - `anti_webui_wayland::Ctx` と `anti_webui_x11::Ctx` に `impl` する。自分のトレイトなので、孤児規則には当たらない。
- **ハンドラはバックエンドごとのまま。**
  - `WaylandHandler` と `X11Handler` は同じ形にする。
  - `Runner` はどちらも、`fn frame(&mut self, ctx: &mut impl Host)` のような共通の関数に一行で渡す。
- **Wayland 独自の言葉は X11 で置き換える。**

  | `Host` のメソッド | Wayland | X11 |
  |---|---|---|
  | `frame_pending`、`last_frame_callback` | フレームコールバック | 最後の `PresentPixmap` の `CompleteNotify` |
  | `surfaces_ready`（先行描画の commit） | 全サーフェスのコールバックが来た | 全ウィンドウの `CompleteNotify` が来た |
  | `dmabuf_feedback` | `zwp_linux_dmabuf_feedback_v1` | DRI3 の `GetSupportedModifiers`（ウィンドウ用と画面用の 2 段） |
  | `present_buffer` | attach、`damage_buffer`、commit | `PresentPixmap` に更新範囲を付ける |
  | `take_presentations`、`presentation_clock` | `wp_presentation` | `CompleteNotify` の `ust` と `msc`（`CLOCK_MONOTONIC`） |
  | `import_timeline` | `wp_linux_drm_syncobj_manager_v1` | DRI3 1.4 の `ImportSyncobj` |
  | `set_opaque` | `set_opaque_region` | 何もしない（ウィンドウは深さ 24）。深さ 32 のものは `_NET_WM_OPAQUE_REGION` |
  | `prepare_commit`、`committed` | wgpu の Surface と commit を分け合う | 何もしない |

まずこの作り替えだけを行い、Wayland での動きと絵が変わらないことを確かめる。それから X11 を足す。

## 使う拡張

| 拡張 | 用途 | 必須 |
|---|---|---|
| コアプロトコル | ウィンドウ、プロパティ、セレクション、ClientMessage | ○ |
| XKEYBOARD | キーマップと修飾キーの状態（libxkbcommon-x11） | ○ |
| XInputExtension 2.2 以上 | ポインタ、なめらかなスクロール。2.4 があればピンチ | ○（無ければコアのボタン 4〜7 で 1 ノッチずつ） |
| DRI3 1.2 以上 | dmabuf から pixmap を作る、modifier の一覧、GPU の特定 | 推奨（無ければ wgpu の Surface） |
| DRI3 1.4 と Present 1.4 | 明示的同期（`ImportSyncobj`、`PresentPixmapSynced`） | 任意（無ければ暗黙的同期） |
| Present | vblank に合わせた提示、更新範囲、画面に出た時刻 | 推奨（無ければ wgpu の Surface） |
| XFIXES | 範囲（Region）、入力を素通りさせる形、セレクションの持ち主の変化 | ○ |
| SYNC | `_NET_WM_SYNC_REQUEST`（リサイズの同期） | 任意 |
| RANDR 1.5 | モニターの範囲（ポップアップを収める）、リフレッシュレート | 任意（無ければルートウィンドウ全体を 1 枚とみなす） |
| SHAPE | コンポジタがないときのポップアップの形 | 任意 |

手元の XWayland 24.1 には、これらがすべてある。

## ウィンドウ

- **見た目：**
  - 深さ 24 の既定のビジュアルで作る。中身は全部自分で描くので、背景は `None` にする。リサイズで広がったところが白く光らない。
  - 透明なウィンドウはしない。必要になったら、深さ 32 と `_NET_WM_OPAQUE_REGION` で足す。
- **窓まわりのプロパティ：**

  | プロパティ | 中身 |
  |---|---|
  | `_NET_WM_NAME`（UTF8_STRING）と `WM_NAME` | タイトル |
  | `WM_CLASS` | `app_id` を、インスタンス名とクラス名の両方に |
  | `WM_PROTOCOLS` | `WM_DELETE_WINDOW`（→ `CloseRequested`）、`_NET_WM_PING`（すぐ返す）、`_NET_WM_SYNC_REQUEST`（WM が `_NET_SUPPORTED` で言うときだけ） |
  | `_NET_WM_PID`、`WM_CLIENT_MACHINE` | WM がプロセスを止めるときのため |
  | `WM_NORMAL_HINTS` | 最初の大きさ |
  | `_NET_WM_WINDOW_TYPE` | `_NET_WM_WINDOW_TYPE_NORMAL` |

- **装飾：** WM に任せる（サーバ側装飾に当たる）。

### リサイズの同期

KWin や Mutter は、`_NET_WM_SYNC_REQUEST` を送ってから大きさを変え、新しい大きさの絵が来るまで枠を描くのを待つ。これに答えると、リサイズ中に枠と中身がずれない。

- SYNC のカウンタを作り、`_NET_WM_SYNC_REQUEST_COUNTER` に置く。
- 要求の値を覚えておく。その後の `ConfigureNotify` の大きさで描いて `PresentPixmap` したら、カウンタをその値にする。

Chromium の `X11Window` も同じことをしている。

## スケールとサイズ

X11 にはウィンドウごとのスケールがない。画面全体で 1 つのスケールを使う。

1. XSETTINGS の `Xft/DPI` を読む（値は DPI × 1024）。
2. なければ、`RESOURCE_MANAGER` の `Xft.dpi` を読む。
3. どちらもなければ 96 DPI とみなす。
4. スケール `n = round(dpi / 96 × 120)`。`ANTIWEBUI_SCALE`（例: `1.5`）で上書きできる。

Chromium も GTK の設定（`Xft/DPI` と `Gdk/WindowScalingFactor`）からスケールを決めている。

| 場面 | すること |
|---|---|
| ウィンドウの最初の大きさ | 物理ピクセル `round(論理 × n / 120)` で作る |
| `ConfigureNotify` | 大きさは物理ピクセルなので、そのまま `Resized { size, scale }` にする。バッファの大きさはウィンドウの大きさと同じで、R3（[01](01-pixel-model.md)）はそのまま成り立つ |
| 設定が変わった（XSETTINGS のウィンドウか、ルートの `RESOURCE_MANAGER` の `PropertyNotify`） | 新しいスケールで `Resized` を出す。ウィンドウの物理の大きさは変えない |

手元の XWayland（ShojiWM）は、XSETTINGS の `Xft/DPI` に 172.8（スケール 1.8）を出し、X のウィンドウを拡大縮小せずにそのまま表示する。そのため、手元でも画素がぴったり合うかを確かめられる。

- `RESOURCE_MANAGER` の `Xft.dpi` は丸めた 173 なので、XSETTINGS を先に見る。
- 起動の直後に設定が切り替わることがある。どちらの変化も見ているので、そのたびに `Resized` で正しいスケールに移る。

## イベントループ

Wayland と同じ形にする。

```
loop {
    溜まっているイベントを全部処理する（poll_for_event が空になるまで）
    アプリの処理
    flush
    poll([X の fd, wake pipe], 次のタイマーまでの timeout)
}
```

- 返事を待つ要求（`GetProperty` など）の間に、libxcb がイベントを先に読んで溜めていることがある。眠る前に必ず溜まった分を空にする。fd が読めなくても、イベントが残っていることがあるためである。
- タイマー：キャレットの点滅など。発火の判定は Wayland と同じく、眠る前に求めた締め切りと比べる。
- キーリピートはサーバーが行うので、自前のタイマーは要らない。

## 入力

### ポインタ

XInput 2 で受ける（`XI_Motion`、`ButtonPress`、`ButtonRelease`、`Enter`、`Leave`）。

- 座標は 16.16 の固定小数で、物理ピクセルの小数まである。そのまま `PointF` にする。ポップアップの上のイベントは、ポップアップの原点を足してウィンドウ座標にする（Wayland と同じ）。
- `XIPointerEmulated` の付いたボタン 4〜7 は、なめらかなスクロールから作られた重複なので捨てる。

### スクロール

XInput 2 のスクロール用の値（valuator）の差分を使う。

- 装置ごとに、前の値と `increment`（1 ノッチの量）を覚える。値は `XIQueryDevice` と `XI_DeviceChanged` で取る。
- `Enter` で前の値を捨てる。ウィンドウの外で動いた分を足さないためである。

| 装置 | source | phase | delta |
|---|---|---|---|
まず値の差を `increment` で割り、ノッチ数にする。

| 装置と動き | source | phase | delta |
|---|---|---|---|
| タッチパッド（`libinput Tapping Enabled` か `Synaptics Off` のプロパティがある） | `Unknown` | `Single` | ノッチ数 × 1 ノッチのピクセル × `AXIS_MOTION_GAIN` |
| それ以外で、ノッチ数が整数（マウスのホイール） | `Wheel` | `Single` | ノッチ数 |
| それ以外で、ノッチ数が整数でない | `Unknown` | `Single` | タッチパッドと同じ |
| XI2 がない | `Wheel` | `Single` | ボタン 4〜7 で ±1 |

- **1 ノッチのピクセル：** libinput のドライバでは 15、XWayland では 10 である。
  - libinput のドライバは、ピクセルの値を `increment` 15 の軸で送る。
  - XWayland は、Wayland の `axis` を 10 で割って送る。
  - XWAYLAND 拡張があるかで、どちらかを選ぶ。
- **整数かどうかで分ける理由：** XWayland の装置はタッチパッドかどうかを言わない。ホイールは 1 ノッチずつ来て、指の動きは端数で来ることで見分ける。
- **見分けの持ち越し：** 端数が来た装置は、300 ms の間は整数でも端数の扱いにする。指の動きがたまたま整数になったときに、ホイールに変わらないようにするためである。

- **タッチパッドに慣性は付けない。**
  - X11 には、指を離したことを知らせるイベント（Wayland の `axis_stop`）がない。
  - Chromium も Firefox も GTK も、X11 のタッチパッドには慣性を付けていない。
  - 時間で指を離したと推測する方法は、途中で止めた指を離したと取り違えるのでやめる。
- ピクセルにした値は Wayland の `axis` と同じ単位なので、Wayland と同じ倍率を掛ける。

### ピンチ

XInput 2.4 の `GesturePinchBegin`、`Update`、`End` を `WindowEvent::Pinch` にする。Xorg 21.1 と XWayland 23 から使える。古いサーバーではピンチは来ない。

### キーボード

- **イベント：** キーはコアの `KeyPress` と `KeyRelease` で受ける（XI2 では受けない）。XIM がコアのキーイベントの形を前提にしているためである。
- **キーマップ：** libxkbcommon-x11 でキーボードの装置からキーマップと状態を作る。
  - `XkbStateNotify` で修飾キーの状態を合わせる。
  - `XkbNewKeyboardNotify` と `XkbMapNotify` で作り直す。
- **リピート：** `XkbPerClientFlags` で DetectableAutoRepeat にする。
  - リピートのたびに偽の `KeyRelease` が挟まらなくなる。
  - 押したままのキーの `KeyPress` が来たら、`repeat: true` にする。
- **フォーカス：** `FocusIn` と `FocusOut` から `Focused` を出す。grab による一時的な移動（`NotifyGrab`、`NotifyUngrab`）や、自分のポップアップへの移動では出さない（Wayland と同じ）。

## カーソル

- `x11rb` の `cursor` を使う。Xcursor のテーマを Rust で読み、`Xcursor.theme`、`Xcursor.size`、`XCURSOR_THEME` に従う。
- `CursorIcon` は CSS の名前で探す。なければ、昔の名前（`left_ptr`、`xterm`、`hand2`、`sb_h_double_arrow` など）で探す。
- 一度作ったカーソルは覚えておき、ウィンドウの属性に付け替えるだけにする。

## 提示（DRI3 と Present）

Wayland の dmabuf presenter と同じく、Vulkan の画像を自分で確保して出す。違いは、バッファを渡す相手が `wl_buffer` ではなく X の pixmap であることだけである。

### バッファ

1. `DRI3 GetSupportedModifiers(window, depth 24, bpp 32)` で、ウィンドウ用と画面用の modifier の一覧を受け取る。これを `DmabufFeedback` の 2 段の tranche にする。
2. 今と同じく、Vulkan で `ARGB8888` の画像を作る（深さ 24 ではアルファは無視される）。
3. `DRI3 PixmapFromBuffers` に、プレーンごとの fd、offset、stride と modifier を渡して pixmap を作る。
   - pixmap の深さは、出すウィンドウの深さに合わせる必要がある（ウィンドウは 24、ポップアップは 32）。
   - `Host::create_dmabuf_buffer` はどこに出すかを知らないので、dmabuf を預かるだけにする。
   - 最初に出すときに、そのウィンドウの深さで pixmap を作る。作れなければ `BufferEvent::Failed` にして、フォールバックに移る。
4. 枚数、作り直し、buffer age の扱いは Wayland と同じ。
   - pixmap の中身は自分のものなので、サーバーがフリップしてもコピーしても、age の数え方は変わらない。

### フレームの流れ

```
空いている pixmap を選ぶ（age が最も小さいもの）
描き直す範囲 = 直近 age フレーム分の変更矩形の和
所有権を取って部分描画し、所有権を返して完了を sync_file にする（Wayland と同じ）
PresentPixmap(window, pixmap, update = 今回の変更矩形の Region, target_msc = 0)
```

- `update` の範囲は、サーバーがコピーするときにその範囲だけを写すのに使われる。X のコンポジタにも、その範囲だけが DAMAGE として伝わる。KWin や picom は、その範囲だけを合成し直す。
- `target_msc = 0` は、次の vblank に出すという意味である。

### 同期

| 方式 | 描画完了の通知 | 再利用の判断 |
|---|---|---|
| 明示的同期（DRI3 1.4 と Present 1.4、`PresentQueryCapabilities` に Syncobj がある） | sync_file を syncobj の acquire の点に取り込み、`PresentPixmapSynced` | release の点が signal 済みか（Wayland と同じ） |
| 暗黙的同期 | sync_file を dmabuf に取り込む | `PresentIdleNotify` を受け、dmabuf から取り出した sync_file を GPU 側で待つ |

- XWayland 24.1 は、コンポジタが明示的同期に対応していれば DRI3 1.4 を出す。手元の環境では明示的同期になる。
- Xorg 21.1（素の X）では暗黙的同期になる。

### 時刻と先行描画

- `PresentCompleteNotify` は、出した時刻（`ust`、マイクロ秒、`CLOCK_MONOTONIC`）と vblank の番号（`msc`）を持つ。これを `Presentation` にして `Pacing` に渡す（[10](10-frame-pacing.md)）。
- リフレッシュの間隔は、RANDR の CRTC のモードから求める。分からなければ、続く 2 つの `msc` の `ust` の差から求める。
- **次のフレームを描いてよいとき：** どのウィンドウにも `CompleteNotify` 待ちがないとき。これが Wayland のフレームコールバックの代わりになる。
  - ポップアップの待ちは 100 ms で打ち切る（Wayland と同じ）。
  - 先行描画で描いておいたフレームは、`CompleteNotify` が来たら `PresentPixmap` する。
- `mode` が `Skip` の `CompleteNotify` は、そのフレームが画面に出なかったことを表す。計測で「出なかった」として数える。

### Expose

コンポジタのない X では、隠れていたところが現れると `Expose` が来る。

- `Host::take_exposed` で presenter に知らせ、次のフレームを全面の変更として描く（Wayland は常に `false`）。
- 最後の pixmap を出し直すだけでも済むが、その pixmap を presenter が次の描画に使い回している最中かもしれない。印を付けるだけの方が安全なのでこうした。めったに起きないので、全面を描く負荷は問題にならない。

### サーバーが答えないとき

- 250 ms たっても `CompleteNotify` が来ない提示は、待つのをやめる。止まったままにならないようにするためである。
- wgpu の Surface で出すときは、`Present` を使うかどうかがドライバ次第である（NVIDIA は使わない）。そのウィンドウで一度でも `CompleteNotify` が来たら待ち、来るまでは待たない。

### フォールバック

次の場合は、wgpu の Surface（`VK_KHR_xcb_surface`、毎回全面を描く）を使う。

- DRI3 1.2 か Present がない。
- NVIDIA のドライバの素の Xorg（DRI3 に対応していない）。
- Vulkan が使えない。
- pixmap を作れなかった。

`ANTIWEBUI_PRESENT=surface` と `ANTIWEBUI_SYNC=implicit` は Wayland と同じく効く。

### 読み込む Vulkan ドライバ

- Wayland の `compositor_main_device` に当たるものとして、`App::run` の最初に X へ短く接続する。
- `DRI3 Open(root, 0)` で受け取った fd を `fstat` して、dev_t を得る。
- あとは同じ `limit_vulkan_drivers` を使う。
- DRI3 がなければ絞らない。

## ポップアップ

X11 にはポップアップの役割がない。上書きリダイレクト（override-redirect）のウィンドウで作り、置き場所も自分で決める。

| 種類 | X11 | 外をクリックすると | 入力 |
|---|---|---|---|
| `Menu` | 上書きリダイレクトのウィンドウとポインタ・キーボードの grab。`_NET_WM_WINDOW_TYPE_DROPDOWN_MENU` か `_POPUP_MENU` | 閉じる | 受ける |
| `Tooltip` | 上書きリダイレクトのウィンドウ。入力の形を空にする（XFIXES `SetWindowShapeRegion`）。`_NET_WM_WINDOW_TYPE_TOOLTIP` | 閉じない | 素通り |
| `Bubble` | メインウィンドウ（親がポップアップならそのウィンドウ）の子ウィンドウ（深さ 32） | 閉じない | 受ける |

- **Bubble を子ウィンドウにする理由：**
  - `wl_subsurface` と同じく、ウィンドウと一緒に動く。
  - 深さの違う子ウィンドウは、サーバーが自動でリダイレクトし、アルファを付けて親に合成する（Composite の別ビジュアルの仕組み）。
  - 実装で、合成の負荷と見た目を確かめる。合わなければ、上書きリダイレクトのウィンドウに替える。
- **深さ：**
  - コンポジタがあるとき（`_NET_WM_CM_S<画面番号>` に持ち主がいるか、XWAYLAND 拡張がある）は、深さ 32 の ARGB ビジュアルで作り、影も描く。
  - ないときは、深さ 24 で作り、SHAPE で形を箱（ジオメトリ）に切る。影はなくなる。
- **ウィンドウとの関係：** `WM_TRANSIENT_FOR` にメインウィンドウを入れる。

### 置き場所

`xdg_positioner` がしていることを自分で行う。

1. アンカーの矩形をルート座標に直す（`TranslateCoordinates`）。
2. `Placement` と `gap` から置き場所を求める。
3. アンカーを含むモニターの範囲に収める。範囲は RANDR の `GetMonitors` と `_NET_WORKAREA` の重なりで決める。
   1. はみ出す向きの反対側に裏返す。
   2. それでもはみ出せば、ずらす。
   3. それでも入らなければ、縮める。
4. 決まった原点と大きさを `PopupConfigured` で返す。往復を待たないので、Wayland より早く出る。

純粋な計算なので、単体テストで確かめる。

### grab

- メニューを開いたら、一番上のメニューのウィンドウにポインタを grab する（XI2 の `XIGrabDevice`、owner events あり）。キーボードはコアの `GrabKeyboard` で grab する。
- owner events なので、自分のウィンドウの上で押したものは、そのウィンドウにそのまま届く。
- それ以外で押したものは grab したメニューに届く。メニューの箱の外（影の部分を含む）なら「外を押した」とする。
- 最後のメニューが閉じたら grab を外す。

### 閉じる経路

| 経路 | X11 での見つけ方 |
|---|---|
| 他のアプリをクリック | grab 中の押下が自分のウィンドウの外。XWayland では Wayland のウィンドウへの押下は届かないので、メインウィンドウの `FocusOut` でも閉じる（Chromium も、ウィンドウが非アクティブになるとメニューを閉じる） |
| 自分のウィンドウを押す | Wayland と同じ |
| Escape、別のメニューを開く | Wayland と同じ |
| メインウィンドウが動いた | `ConfigureNotify` の位置が変わったら `Menu` と `Tooltip` を閉じる。上書きリダイレクトのウィンドウはついてこないためである |

## クリップボード

CLIPBOARD と PRIMARY のセレクションを使う。読み書きはイベントループの中で行い、スレッドは使わない。

- **コピー：** 最後のユーザー操作の時刻で `SetSelectionOwner` する。`SelectionRequest` には、次の形式で答える。
  - `TARGETS`、`TIMESTAMP`
  - `UTF8_STRING`、`text/plain;charset=utf-8`、`text/plain`
  - `STRING`（Latin-1 に変換）、`TEXT`
- **大きなデータ：** 要求の上限を超えるものは INCR で分けて送る。
- **貼り付け：**
  1. `ConvertSelection` で自分のウィンドウのプロパティに入れてもらう。
  2. `SelectionNotify` で読み、INCR なら `PropertyNotify` で続きを読む。
  3. 自分が持ち主なら、手元の文字をそのまま返す（Wayland と同じ）。
  4. 5 秒で返事がなければ、空として扱う。
- **終わるとき：** 持ち主のままアプリを閉じると、クリップボードの中身が消える。`CLIPBOARD_MANAGER` がいれば、`SAVE_TARGETS` で中身を預ける（Chromium と GTK も同じ）。

## ドラッグ＆ドロップ

XDND バージョン 5 を使う。

### 受ける側

- メインウィンドウに `XdndAware = 5` を置く。
- ClientMessage を、core のイベントと答えに対応させる。

  | ClientMessage | すること |
  |---|---|
  | `XdndEnter` | `XdndTypeList`、または中の 3 つの型（形式名は MIME 型）を受け取り、`DragEntered` を出す |
  | `XdndPosition` | ルート座標をウィンドウ座標に直して core に渡し、`AcceptDrag` の答えで `XdndStatus` を返す |
  | `XdndLeave` | 離れたことを core に伝える |
  | `XdndDrop` | `ReceiveDrop` で `XdndSelection` を `ConvertSelection` して読み、`XdndFinished` を返す |

### 渡す側

1. `StartDrag` でポインタとキーボードを grab する。
2. 動くたびに、ポインタの下のトップレベルを探す。ルートから子へ `TranslateCoordinates` でたどり、`XdndAware`（版 3 以上）のあるウィンドウで止める。ドラッグの絵のウィンドウは飛ばす。
3. 相手には `XdndEnter`、`XdndPosition`、`XdndLeave` を送り、`XdndStatus` で受け取るかどうかを知る。
4. 離したとき、受け取ると言っていれば `XdndDrop` を送る。
5. `XdndSelection` の `SelectionRequest` に `DragOffer` の中身で答え、`XdndFinished` で `DragSourceEnded` を出す。
6. Escape で取りやめる。

- **ドラッグの絵：** 上書きリダイレクトの ARGB ウィンドウにする。`_NET_WM_WINDOW_TYPE_DND` を付け、入力の形を空にして、ポインタに合わせて動かす。
- **自分のウィンドウの上：** プロトコルを通さず、`DragEntered`、`DragMoved`、`DragLeft`、`DragDropped` を直接 core に渡す。
  - core が `StartDrag` を頼むのは、遊びを越えたときである（[23](23-drag-and-drop.md)）。それからは自分の窓の上でも、このイベントで動く。Wayland でコンポジタが自分に送り返すのと同じことになる。
  - 落としたあと、`ReceiveDrop` には空のデータを返す（core は手元の値を使う）。`FinishDrop` で、core が受け付けた動作から `DragSourceEnded` を出す。
- **位置の答え：** XDND では、`XdndPosition` のたびに `XdndStatus` を返す決まりである。
  - core は受け手が変わったときしか `AcceptDrag` を頼まない。
  - そのため、イベントを core に渡し終えたところで、まだ答えていない位置があれば、今の受け付けの状態で答える。

Chromium の `x11_drag_drop_client` と同じ形になる。

## IME

XIM を使う。fcitx5 も IBus も XIM のサーバーになるので、どちらでも動く。XWayland の上でも、fcitx5 が XIM を出している。

- **ライブラリ：** `xim` クレートの x11rb クライアントを使う（MIT）。接続は IME と共有するので、`Rc<XCBConnection>` にした。
- **スタイル：** on-the-spot（`XIMPreeditCallbacks | XIMStatusNothing`）。
  - 変換中の文字は、`preedit_draw` で受け取って core に渡す。
  - 下線などの飾り（feedback）も渡す。
- **キー：**
  - IC にフォーカスがある間は、キーイベントを先に XIM へ送る（`ForwardEvent`、非同期）。
  - XIM が使わずに戻したキーだけを core に渡す。
  - 確定した文字は `commit` として渡す。
- **候補の位置：** `SpotLocation`（ウィンドウ座標、物理ピクセル）に、カーソル矩形の左下を入れる。
- **状態の対応：**

  | core の状態 | XIM |
  |---|---|
  | 編集できる要素にフォーカス | IC に `SetICFocus` |
  | フォーカスが外れた、`SetIme(None)` | `UnsetICFocus` |
  | `ImePurpose::Password` | IC を外す（Chromium も、パスワード欄では IME を切る） |

- **XIM では渡せないもの：**
  - 周りの文字（surrounding text）がないので、再変換などは使えない。
  - 入力の種類（数字、URL など）もない。
  - これらは text-input-v3 より劣る。
- **XIM のサーバーがないとき：** `XMODIFIERS` がない、またはつながらないときは、xkb だけで文字にする。

Chromium は、GTK の入力メソッドを通している。GTK を使わないので、同じ形にはしない。fcitx5 や IBus の D-Bus に直接つなげば、周りの文字も入力の種類も渡せる。ただし D-Bus のクライアントが要るので、必要になったら別の機能として足す。

## フルスクリーン

- **切り替え：** ルートに `_NET_WM_STATE` の ClientMessage（`_NET_WM_STATE_ADD` か `_REMOVE` と `_NET_WM_STATE_FULLSCREEN`）を送る。
- **起動時：** `App::fullscreen(true)` なら、マップする前に `_NET_WM_STATE` のプロパティに入れておく。
- **知らせる：** 実際に変わったかは、`_NET_WM_STATE` の `PropertyNotify` で知り、`WindowEvent::Fullscreen` を出す（[27](27-fullscreen.md)）。
- **フルスクリーンの間：** `_NET_WM_BYPASS_COMPOSITOR = 1` を置く。X のコンポジタが合成をやめ、Present がフリップできる。

## Chromium との比較

| | Chromium（X11） | これ |
|---|---|---|
| プロトコル | 自前の生成コード（`ui/gfx/x`） | `x11rb`（同じ xcbproto からの生成） |
| 入力 | XInput 2 | XInput 2（キーだけコア） |
| 提示 | GL か Vulkan のスワップチェーン（中で Mesa が DRI3 と Present を使う） | 自分で DRI3 と Present。更新範囲を渡し、buffer age で部分描画する |
| スケール | GTK の設定（`Xft/DPI`） | XSETTINGS、`Xft.dpi` |
| IME | GTK の入力メソッド | XIM |
| クリップボード | セレクション、INCR、`SAVE_TARGETS` | 同じ |
| DnD | XDND 5 | 同じ |
| リサイズ | `_NET_WM_SYNC_REQUEST` | 同じ |
| メニュー | 上書きリダイレクトのウィンドウ、非アクティブで閉じる | 同じ |
| タッチパッドの慣性 | ない | ない |

## 確かめ方

- **単体テスト（サーバーなし、15 件）：**
  - ポップアップの置き場所（下、上、中央、裏返し、ずらし、縮め、横）
  - XSETTINGS の読み取りとスケール
  - スクロールの変換
  - カーソル名の探し方
  - Present の完了の対応付け（飛ばされたフレーム、リフレッシュの間隔）と、Surface の待ち方
  - Latin-1 の変換
- **X サーバーを使うテスト（`tests/server.rs`、8 件）：**
  - テストが `Xvfb :93` を起動し、イベントを `XTEST` で流す。Xvfb がなければ飛ばす。
  - `-terminate` で起動し、テストの間は接続を 1 本持ち続ける。テストのプロセスが終わると Xvfb も終わる。
  - Xvfb には DRI3 がなく、RADV の Vulkan も Xvfb には出せないので、描かずにバックエンドだけを確かめる。
  - 確かめていること：
    - `Xft.dpi` 144 で 400 × 300 が 600 × 450 になる
    - キーの文字とリピート
    - 押した位置とボタン、ホイールのノッチ
    - 1 MB 余りのクリップボードを INCR で渡す、他のクライアントから貼り付ける
    - メニューの位置、外を押すと閉じる
    - `WM_DELETE_WINDOW`
    - 外からのドロップ（`XdndStatus`、読み取り、`XdndFinished`）
    - 外へのドラッグ（`XdndEnter`、`XdndPosition`、読み取り、`DragSourceEnded(Copied)`）
- **手元の XWayland での確認（済み）：**
  - showcase を `ANTIWEBUI_BACKEND=x11` で起動すると、DRI3 と明示的同期の経路で描けた。
  - grim の画面写真で、WM の枠の中に正しく出ていることを確かめた。
- **手での確認：**
  - 手元の XWayland で、showcase、Markdown エディタ、PDF を動かす。
  - Chromium（`--ozone-platform=x11`）とフレームの時間を比べる。
  - `ANTIWEBUI_PERF_OVERLAY=1` で、提示の経路（DRI3 か Surface か）と同期の方式を見る。
- **素の Xorg：**
  - 暗黙的同期の経路とコンポジタのないポップアップは、別の VT で Xorg のセッションを起動して確かめる（amdgpu、glamor で DRI3 がある）。

## 段階

すべて 2026-11-07 に作った。

| 段階 | 中身 | 終わったら |
|---|---|---|
| X0 | app の作り替え（`Host`、型を core へ）、バックエンドの選び方 | Wayland の絵とテストが変わらない |
| X1 | ウィンドウ、イベントループ、ポインタ、スクロール、キーボード、スケール、カーソル、タイトル、閉じる、フルスクリーン、wgpu の Surface での提示 | X11 でアプリが使える |
| X2 | DRI3 と Present の提示、明示的同期と暗黙的同期、時刻と先行描画、Vulkan ドライバの絞り込み、リサイズの同期 | Wayland と同じ経路で描ける |
| X3 | ポップアップ（メニュー、ツールチップ、バブル）、クリップボードとプライマリ | メニューと貼り付け |
| X4 | IME（XIM）、ドラッグ＆ドロップ、ピンチ | Wayland でできることが揃う |

## やらないこと

- **モニターごとのスケール。** X11 にはない。Chromium も画面全体で 1 つである。
- **タッチパッドの慣性。** 上の「スクロール」を参照。
- **タッチスクリーン。** Wayland でもまだしていない。
- **透明なメインウィンドウ、クライアント側の装飾。**
- **D-Bus の IME（fcitx5、IBus に直接）。** 必要になったら別の機能にする。
- **GLX と EGL での提示。** Vulkan がなければ、wgpu の GL バックエンドの Surface に任せる。
- **読み上げ。** AT-SPI は X11 でも Wayland でも同じなので、[34](34-accessibility.md) の橋渡しのクレートがそのまま使える。
- **`XdndProxy`。** デスクトップのアイコンなど、代理のウィンドウで受けるものには落とせない。
- **XIM の失敗からの立ち直り。** IME のサーバーが途中で落ちたときに、つなぎ直すことはしない。
- **クリップボードの `MULTIPLE`。** 対応している形式を一度に頼む古い要求には答えない。
