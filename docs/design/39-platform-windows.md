# 39. Windows バックエンド

Wayland、X11、macOS でできていることを、Windows でも同じ書き味で動かす。描く仕組み（core、レンダラ、`Runner`、`Presenter`、`Pacing`）はそのまま使い、ウィンドウ、入力、提示、ポップアップ、クリップボード、IME を Win32 につなぐ。

## 方針

- **Win32 を直接使う。** `winit` などは使わない。Wayland、X11、macOS と同じく、`Session`、`Ctx`、ハンドラの形をそろえ、`app` の `Host` と `Platform` に一行ずつ足すだけでつながるようにする（[35](35-platform-x11.md)、[38](38-platform-macos.md) の「app の作り替え」）。
  - 呼び出しは `windows` クレート（0.62）を使う。wgpu の DX12 が同じ版をすでに使っているので、依存は増えない。
  - Chromium も `ui/views` の下は自前の `HWND`（`HWNDMessageHandler`）で、コモンコントロールは使わない。
- **イベントループは今の形のまま。** `GetMessage` に制御を渡し切らず、`dispatch`（溜まった入力を渡して描く）と `wait`（締め切りまで眠る）を `Thread::run` が回す。眠るのは `MsgWaitForMultipleObjectsEx` で、メッセージ、起こす合図（イベントオブジェクト）、締め切りのどれかで起きる。
- **ウィンドウはそれを作ったスレッドのもの。** Win32 のウィンドウとメッセージキューはスレッドごとなので、`spawn_window` の窓は自分のスレッドで作り、そのスレッドで回す（`OWN_THREADS = true`）。macOS のように別のスレッドへ頼みごとを回す仕組みは要らない。
- **DPI はモニターごと（Per-Monitor v2）。** プロセスの初めに `SetProcessDpiAwarenessContext(PER_MONITOR_AWARE_V2)` を呼ぶ。座標はすべて物理ピクセルで受け取り、スケールは `GetDpiForWindow` から決める。
- **描画は wgpu の Surface（DX12）を DirectComposition の上に。** 最初の段階では自前のバッファを回さず、wgpu の Surface に描く（[05](05-rendering.md) のフォールバックの経路）。中身が変わらなければ描かないのは他と同じ。
- **キーの決まりは Linux と同じ。** Ctrl がショートカットの修飾キーで、単語単位の移動も Ctrl である。core の `Modifiers` はすでに macOS 以外をこの形にしている。

## バックエンドの選び方

Windows では `windows` だけを使う。`ANTIWEBUI_BACKEND` は空か `windows` のみ受け付け、他はエラーにする。Cargo の機能 `windows`（既定で有効）で外せる。`wayland`、`x11`、`macos` のクレートは Windows では中身が空になるので、`cargo check --workspace` は Windows でも通る。

## Windows でビルドが通るようにしたこと

| 場所 | 変更 |
|---|---|
| `core::platform` | dmabuf の口（`DmabufPlaneDesc`、`Host::import_timeline`）が `std::os::fd::BorrowedFd` を使っていた。Unix 以外では、値を作れない同名の型を置く。Linux の presenter しか呼ばないので、Windows でこれが作られることはない |
| `core::drag` | `text/uri-list` とパスの変換。Windows では `C:\a\b` を `file:///C:/a/b` にし、逆も行う |
| `anti-webui-video` | dmabuf への書き出しは Unix のみ。Windows ではメモリに読み戻す経路だけになる（FFmpeg の DLL の名前の扱いは今後） |

## app の作り替え

| 場所 | 変更 |
|---|---|
| `host.rs` | `host_for!(anti_webui_windows::Ctx<'_>)`。`BorrowedFd` は core のものを使う |
| `runner.rs` | `handler_for!`、`platform_for!`、`Backend::Windows`、`Link::Windows`、Windows の `choose_backend` |
| `clock.rs` | Windows は `QueryPerformanceCounter` をナノ秒にしたもの。`Instant` も同じ時計で、提示の時刻もこの時計で渡す（`presentation_clock` は `Some(0)`、`clock_ns` は時計の番号を見ない） |
| `lib.rs` | Linux と macOS に限っていたモジュールを Windows でもビルドする |
| `present.rs` | 変えない。Windows の自前のバッファは「値を持たない型」（`present/no_dmabuf.rs`）で、いつも wgpu の Surface になる |
| `wgpu` の `with_window` | Windows では DX12 のスワップチェーンを `DxgiFromVisual`（DirectComposition のビジュアル）で作る。`DxgiFromHwnd` は `Opaque` しか持たず、ポップアップの角と影が透けないためである。`WGPU_DX12_PRESENTATION_SYSTEM` があればそれに従う |
| `pacing.rs` | GPU のフェンス（sync file）の型は Unix のみ。他では値を作れない型にする |
| `runner.rs` の `run` | 全体を 8 MiB のスタックで回す（後述の「スタック」） |

## ウィンドウ

- ウィンドウクラスは 1 つ（`AntiWebUI`）。プロセスで 1 回だけ登録する。背景のブラシは持たず、`WM_ERASEBKGND` は何もしない。
- 拡張スタイルに `WS_EX_NOREDIRECTIONBITMAP` を付ける。GDI のリダイレクトの面を作らず、中身は DirectComposition のビジュアルだけになる。ポップアップが透けるのはこのためで、メインウィンドウも同じにする（Chromium も DirectComposition で描くときは同じ）。
- 大きさは論理ピクセルで渡される。開く画面の DPI で物理ピクセルにし、`AdjustWindowRectExForDpi` で枠を足して `SetWindowPos` する。位置は `CW_USEDEFAULT`（Windows の重ね方）に任せ、親（ダイアログ）があるときは親の中央にする。
- `parent`（ダイアログとモーダル）は「所有者」（`CreateWindowEx` の `hWndParent`）にする。所有されたウィンドウは常に所有者の上にあり、所有者と一緒に最小化される。`ParentLink` は `HWND` の値。
  - 所有者が別のスレッドのウィンドウだと、二つのスレッドの入力キューがつながる（`AttachThreadInput` と同じ）。これは Win32 の決まりで、避けない。
- `WM_CLOSE` は閉じずに `CloseRequested` を出す。閉じる前のハンドラを通り、`Session::close` が `DestroyWindow` する（[36](36-multi-window.md)）。
- 最大化と最小化：`ShowWindow(SW_MAXIMIZE / SW_RESTORE / SW_MINIMIZE)`。`WM_SIZE` の `SIZE_MAXIMIZED` を見て `Maximized` を出す。
- フルスクリーン：枠のない全面（ボーダーレス）にする。スタイルを `WS_POPUP` にし、ウィンドウのあるモニターの全体に `SetWindowPos` する。前のスタイルと `WINDOWPLACEMENT` を覚えておき、戻すときに使う。排他的なフルスクリーンは使わない（Chromium も同じ）。

### 窓の飾り

| `DecorationMode` | Windows |
|---|---|
| `Auto`、`Server` | システムの枠（`WS_OVERLAPPEDWINDOW`）。`Decorations::Server` を出す |
| `Client` | スタイルは `WS_OVERLAPPEDWINDOW` のまま、`WM_NCCALCSIZE` でクライアント領域を窓全体に広げる。スナップ、最大化のアニメーション、影はシステムのものが残る。`Decorations::Client` を出す |

- `Client` で最大化したときは、窓が画面の外に枠の太さだけはみ出すので、`WM_NCCALCSIZE` でその分を内側に縮める。
- `Client` のリサイズの縁：`WM_NCHITTEST` で、窓の端から枠の太さ（`GetSystemMetricsForDpi(SM_CXSIZEFRAME) + SM_CXPADDEDBORDER`）の内側を `HTLEFT` などにする。それ以外は `HTCLIENT` で、タイトルバーの部品（`title_bar`）に届く。
- `start_move`：`ReleaseCapture` のあと `WM_NCLBUTTONDOWN(HTCAPTION)` を送る。Windows が移動のループに入る（Aero Snap も効く）。
- `start_resize`：同じく `WM_NCLBUTTONDOWN(HTLEFT など)`。
- `show_window_menu`：`GetSystemMenu` を `TrackPopupMenu` で出し、選ばれたものを `WM_SYSCOMMAND` で送る。

## スタック

Windows のメインスレッドのスタックは 1 MiB で、Linux と macOS（8 MiB）より小さい。デバッグビルドでは、大きなビュー（`widgets` の例）を組むと 1 MiB を超えて落ちた（2 MiB で足りることを確かめた）。

- `App::run` は、呼ばれたスレッドのまま、8 MiB のファイバーに切り替えて全体を回す（`with_stack`、`stack.rs`）。`ConvertThreadToFiber` と `CreateFiberEx` を使う。
  - 別のスレッドに移すと、`root` が `Send` でなければならなくなる。ファイバーならスレッドは同じなので、ウィンドウ、メッセージキュー、スレッドローカルはそのまま使える。
  - パニックはファイバーの中で受け止め、元のファイバーに戻ってから投げ直す。
  - すでにファイバーのスレッド（他のライブラリが変えたもの）では、そのまま回す。
- ユーザーにリンカーの指定（`/STACK`）を求めない。
- `spawn_window` のスレッドは他の OS と同じ（Rust の既定の 2 MiB）。

## スケールとサイズ

- スケールは `Scale(dpi × 120 / 96)`。96、120、144、192 DPI（100%、125%、150%、200%）は、どれも 120 分の整数で割り切れる（120、150、180、240）。175% などもそのまま正確に表せる。
- バッファの大きさはクライアント領域の物理ピクセル（`WM_SIZE`）。R3（[01](01-pixel-model.md)）のとおり、これをそのままルートの制約にする。
- 別の DPI の画面に移ると `WM_DPICHANGED` が来る。Windows が勧める矩形（`lParam`）に `SetWindowPos` し、`Resized` を新しいスケールで出す。
- 最小化中（`SIZE_MINIMIZED`）は大きさが 0 になるが、`Resized` は出さず、描かない。戻ったら全面を描き直す（`take_exposed`）。

## イベントループ

```
loop {
    dispatch: 各ウィンドウの受け箱のイベントを渡す、タイマー、描く
    命令（開く、閉じる）
    wait: MsgWaitForMultipleObjectsEx([起こす合図], 締め切り, QS_ALLINPUT)
          → PeekMessage / TranslateMessage / DispatchMessage を溜まっている分だけ
}
```

- **受け箱：** ウィンドウプロシージャは `WindowEvent` を作って、ウィンドウの受け箱（`Rc<Shared>`、`GWLP_USERDATA` に置く）に入れるだけにする。`State` には触らない。
  - Win32 はこちらの呼び出しの途中で同じウィンドウのプロシージャを呼び返す（`SetWindowPos` の中の `WM_SIZE` など）。`State` を借りたまま呼び返されても壊れないようにするためである。macOS と同じ形。
  - その場で答えを返さなければならないもの（`WM_NCHITTEST`、`WM_NCCALCSIZE`、`WM_SETCURSOR`、`WM_GETMINMAXINFO`、IME の文字列）は、`Shared` の値だけで答える。
- **起こす：** 他のスレッド（画像のデコード、`ui_channel`、`shared_signal`、垂直同期のスレッド）は、セッションのイベントオブジェクトを `SetEvent` する。`MsgWaitForMultipleObjectsEx` が返る。メッセージを投げる（`PostThreadMessage`）より、モーダルなループの中でも消えない。
- **モーダルなループ（ライブリサイズ）：** 窓の縁や枠をドラッグしている間、`DefWindowProc` は自分のループを回し、こちらの `wait` に戻ってこない。macOS のライブリサイズと同じ問題である。
  - `wait` は `DispatchMessage` を呼んでいる間だけ、自分の `dispatch` を呼ぶ口（ポンプ）をスレッドローカルに置く。
  - `WM_ENTERSIZEMOVE` で窓にタイマー（`SetTimer`、およそ 1 フレーム）を付け、`WM_EXITSIZEMOVE` で外す。`WM_SIZE` と、ループ中の `WM_TIMER` はポンプを呼ぶ。その場で新しい大きさの絵が描かれ、アニメーションも止まらない。
  - ポンプは使っている間は外しておく。入れ子にはならない。
- **キーボードのメッセージ：** ループで `TranslateMessage` を呼び、`WM_CHAR` を作らせる。`WM_KEYDOWN` を受けたら、キューの次にある `WM_CHAR` を取り出して、そのキーの `text` にする（後述）。

## 提示

- **wgpu の Surface（いまの既定）：** `present/no_dmabuf.rs` の presenter は値を持たないので、`Presenter` はいつも wgpu の Surface に描く。DX12 の flip model のスワップチェーンを DirectComposition のビジュアルに付けたもの（`Dx12SwapchainKind::DxgiFromVisual`）で、`PreMultiplied` のアルファを持てる。
  - 提示のモードは `Mailbox`（`Present(0, 0)`）。DWM が次の合成で最新の 1 枚を使う。描く間隔はこちらの垂直同期の合図で決めるので、描きすぎない。
  - ポップアップの面も、それぞれの `HWND` に同じ方法で Surface を作る（`raw_handles`）。GPU は共有する。
  - **DX12 を最初に試す。** wgpu の既定（`PRIMARY`）では Vulkan が選ばれることがあり（Radeon 880M で起きた）、Vulkan の Win32 のスワップチェーンは不透明なので、ポップアップの影の部分が黒くなった。Windows では DX12、`PRIMARY`、GL の順に試す。`WGPU_BACKEND` があればそれに従う。
- **残す絵とスワップチェーンの余裕：** スワップチェーンを作り直す（`configure`）と、GPU の仕事が全部終わるのを待つ。Radeon 880M で 1 回 25 ms かかり、ライブリサイズは 1 フレーム 30〜45 ms（20〜30 fps）になっていた。
  - フレームは、ウィンドウと同じ大きさのテクスチャ（`WindowRenderer` の `retained`）に描き、それをスワップチェーンの左上に写す（`copy_texture_to_texture`）。テクスチャは大きさが変わったら作り直すが、待ちは無い。
  - スワップチェーンは 256 px 単位で大きめに取り、足りなくなったときと、512 px 以上余ったときだけ作り直す。余った部分は DirectComposition がウィンドウの外として切る（画面で確かめた）。
  - 残したテクスチャには前のフレームが入っているので、変わったところだけ描く（`render_damaged`）。表のスクロールで、描く画素が 100% から 40% になった。
  - スワップチェーンが写しを受けない（`COPY_DST` が無い）ときは、前のとおり全面を直に描く。
  - 計測（リリースビルド、1000×700 の論理サイズ、175%、Radeon 880M、120 Hz）：

| | 前 | 後 |
|---|---|---|
| ライブリサイズの 1 フレーム（p50） | 28.9 ms | 1.4 ms（作り直すときだけ約 38 ms） |
| ライブリサイズのフレームの間隔（p50） | 33.3 ms | 8.3 ms |
| 表のスクロールで描く画素 | 100% | 40% |

- **フレームの間隔：** 垂直同期のスレッド（セッションごとに 1 本）が `IDXGIOutput::WaitForVBlank` で待ち、来たらセッションを起こす。Chromium の `GpuVSyncWorker` と同じ考え方で、macOS の `CADisplayLink` と同じ役目をする。
  - 待つ出力は、セッションの最初の見えているウィンドウのあるモニター（`MonitorFromWindow` → DXGI の出力の `Monitor` と照らす）。
  - 提示したら「待ち」にし、垂直同期のスレッドを動かす。次の垂直同期が来たら「待ち」を解き、描き直しが要るならすぐ描く。
  - 入力が来て、待ちでなければ、次の垂直同期を待たずにすぐ描く。
  - 待つものが無ければ、スレッドは条件変数で眠る。止まっている間は CPU を使わない。
  - DXGI の出力が見つからないとき（リモートデスクトップなど）は `DwmFlush` で待つ。それも失敗すれば 16 ms ごと。
  - 垂直同期が来ないときのため、待ちは 100 ms で諦める。
- **画面に出た時刻：** 提示のあとの次の垂直同期の時刻（`QueryPerformanceCounter`）を画面に出た時刻とみなし、周期（モニターの `dmDisplayFrequency`）と合わせて `Pacing` に渡す。macOS と同じ近似で、DWM が 1 回遅れたときはそれだけずれる。
- **隠れているとき：** 最小化中（`IsIconic`）は描かない。戻ったら全面を描き直す。
- **最初のフレームまで隠す：** `WS_EX_NOREDIRECTIONBITMAP` のウィンドウには GDI の面が無いので、最初の提示までクライアント領域は透明になる。そのまま見せると、開くときに枠（`Client` の飾りでは影だけ）が中身なしで一瞬映る。デバッグビルドの `widgets` で約 760 ms、wgpu の Surface を作って最初のフレームを描くまでの間だった。
  - そこで、`ShowWindow` の前に `DWMWA_CLOAK` で隠し、最初の提示の後の垂直同期が来たら（`take_tick` が提示を受けたら）出す。見えている扱いなので、アクティブになり、大きさも決まり、Surface も作れる。Windows Terminal も同じことをする。
  - フレームが来ないとき（描画が失敗したなど）のため、2 秒で出す。
  - ポップアップは隠さない。透明な間は何も映らないだけで、枠が無いので目立たない。
  - 画面の画素を 5 ms ごとに読んで確かめた：隠れている間は後ろのものが映り、出たときにはもう中身（白）が映っている。
- **今後（自前のバッファ）：** [05](05-rendering.md) の表のとおり、DXGI のスワップチェーンを自分で持ち、`IDXGISwapChain1::Present1` の dirty rects で変わったところだけを DWM に渡す。wgpu の Surface は変わった矩形を渡す口を持たないので、`create_texture_from_hal` で包む自前の presenter（`present/dxgi.rs`）にする。buffer age は flip model の `GetCurrentBackBufferIndex` から求める。

## 入力

| Win32 | `WindowEvent` |
|---|---|
| `WM_MOUSEMOVE` | `PointerMoved`。初めの 1 回で `TrackMouseEvent(TME_LEAVE)` |
| `WM_MOUSELEAVE` | `PointerLeft`。ただしポインタが自分のポップアップ（または自分のウィンドウ）に移っただけなら出さない |
| `WM_[LRM]BUTTON(DOWN/UP)`、`WM_XBUTTON*` | `PointerButton`。押したら `SetCapture`、全部離したら `ReleaseCapture`。時刻は `GetMessageTime` |
| `WM_MOUSEWHEEL`、`WM_MOUSEHWHEEL` | `Scroll`（`ScrollSource::Wheel`、`ScrollPhase::Single`）。量はノッチ（`WHEEL_DELTA` = 120 で 1）。縦は符号を反転する。座標は画面座標なので `ScreenToClient` する |
| `WM_KEYDOWN`、`WM_SYSKEYDOWN`、`WM_KEYUP`、`WM_SYSKEYUP` | `Key` |
| `WM_CHAR`（キーに続かないもの） | `Ime`（`commit`） |
| `WM_SETFOCUS`、`WM_KILLFOCUS` | `Focused` |
| `WM_CLOSE` | `CloseRequested` |

- ポップアップの上のポインタの座標は、ポップアップの原点（ウィンドウ座標）を足して、ウィンドウ座標にする。Wayland と同じく、core にはどの面の上かを渡さない。
- 精度の高いタッチパッドは DirectManipulation で受ける（次の節）。DirectManipulation が無いときや、受け取られなかった指は、`WM_MOUSEWHEEL` の小さな量（ノッチの端数）として来て、`Wheel` として扱う。

### タッチパッド

`WM_MOUSEWHEEL` だけだと、精度の高いタッチパッドのスクロールが滑らかにならない。

- Windows が `WM_MOUSEWHEEL` を送る間隔は、画面のリフレッシュとは関係なく、揃っていない。フレームごとの量がばらつき、来ないフレームもある。スクロールバーをつかむ操作（`WM_MOUSEMOVE`）は滑らかなのに、タッチパッドだけ遅く見えるのはこのため。
- 指が触れた・離れたが分からず（`ScrollPhase::Single`）、Windows の慣性も指の動きと区別できない。
- ピンチは Ctrl 付きのホイールになる。

そこで Chromium と同じく DirectManipulation を使う（`manipulation.rs`）。

- ウィンドウごとに `IDirectManipulationManager` とビューポートを作る。ビューポートは描かない（1000×1000）。設定は平行移動（X、Y、慣性、レール）と拡大縮小。`MANUALUPDATE` にして、こちらが頼んだときだけ変換を進める。
- 指が置かれると `DM_POINTERHITTEST` が来る。ポインタの種類が `PT_TOUCHPAD` なら `SetContact` でビューポートに渡し、ウィンドウはその指の `WM_MOUSEWHEEL` を受けなくなる。
- ビューポートが動いている間（`RUNNING`、`INERTIA`）は、垂直同期のスレッドに毎回の垂直同期を頼み、そのたびに `IDirectManipulationUpdateManager::Update` で 1 回だけ変換を進める。指が置かれてから `RUNNING` になるまでの間も、最大 30 回は進める。こうして、フレームごとに 1 回、時刻の揃った量が来る。
- 内容の変換（平行移動と倍率）の差を、物理ピクセルの `Scroll`（`ScrollSource::Touchpad`）か `Pinch` にする。ジェスチャーがどちらになるかは最初の動きで決める（倍率が変われば `Pinch`）。

| ビューポート | `WindowEvent` |
|---|---|
| `RUNNING` になって最初の動き | `Scroll`（`Began`）または `Pinch`（`Began`） |
| `RUNNING` の間の動き | `Scroll`（`Moved`）または `Pinch`（`Moved`） |
| `RUNNING` → `INERTIA` | `Scroll`（`Lifted { fling: false }`）。慣性は Windows が計算する。ピンチは `Ended` |
| `INERTIA` の間の動き | `Scroll`（`Momentum`） |
| `INERTIA` → `RUNNING`（指が戻った） | `Scroll`（`Held`）。慣性が止まり、次の動きが新しい `Began` になる |
| → `READY` | `Scroll`（`MomentumEnded`。慣性なしで離したときは先に `Lifted`）、ピンチは `Ended` |

- `READY` になったら、変換を `ZoomToRect` で元に戻し、次のジェスチャーが単位行列から始まるようにする。戻す間の変換の知らせは無視する。
- 座標は `GetCursorPos`、時刻は `GetTickCount`（`GetMessageTime` と同じ時計）。
- COM はウィンドウのあるスレッドごとに `CoInitializeEx(COINIT_APARTMENTTHREADED)` で始める。DirectManipulation が作れなければ、ホイールのままにする。
- 状態の移り変わりから出来事への変換は、単体テストで確かめている。実際のタッチパッドの指は合成できない（`CreateSyntheticPointerDevice` はタッチとペンだけ）ので、E2E テストではビューポートが作られることだけを見ている。

### キーボード

- `key`：`VK_*` から決まるもの（Enter、矢印、F1–F24、修飾キーなど）は `NamedKey`。それ以外は、Ctrl と Alt を除いた状態（Shift と CapsLock は効かせる）で `ToUnicodeEx` を呼んだ文字。キーボードの状態を変えない（デッドキーを壊さない）フラグ `0x4` を付ける。
- `text`：`TranslateMessage` が作った `WM_CHAR` を、`WM_KEYDOWN` の処理の中で `PeekMessage(PM_REMOVE)` して集める。UTF-16 のサロゲートの組もここでつなぐ。制御文字は捨てる。`WM_DEADCHAR` は文字にしない。
- AltGr（右 Alt）は Windows では Ctrl+Alt として届く。その組で文字が打たれたときは、`ctrl` と `alt` を外して渡す（`types_text` が真になる）。
- `WM_SYSKEYDOWN` は `DefWindowProc` に渡さない。Alt を離したときにメニューバーのモードに入り、次のキーを取られるからである。ただし Alt+F4 と Alt+Space は `DefWindowProc` に渡し、窓を閉じる、システムメニューを出す、が効くようにする。`WM_SYSCHAR` は捨てる（ビープを鳴らさない）。
- 繰り返しは `lParam` の 30 ビット目。
- IME が受けたキーは `VK_PROCESSKEY` で来る。これは core に渡さない。

## IME

IMM32 を使う（TSF は今後）。Chromium も長く IMM32 で、TSF に移ったのは最近である。

| IMM32 | core |
|---|---|
| `WM_IME_SETCONTEXT` | `ISC_SHOWUICOMPOSITIONWINDOW` を外して `DefWindowProc` へ。未確定文字列はこちらでインラインに描く |
| `WM_IME_STARTCOMPOSITION` | 何もしない（`DefWindowProc` に渡さず、既定の入力窓を出さない） |
| `WM_IME_COMPOSITION` の `GCS_RESULTSTR` | `Ime { commit }` |
| `WM_IME_COMPOSITION` の `GCS_COMPSTR` | `Ime { preedit: (文字列, 範囲) }`。範囲は変換中の文節（`GCS_COMPATTR` の `ATTR_TARGET_CONVERTED` / `ATTR_TARGET_NOTCONVERTED`）、無ければカーソル（`GCS_CURSORPOS`）。UTF-16 の位置をバイトにする |
| `WM_IME_ENDCOMPOSITION` | `Ime { preedit: None }` |

- `set_ime(Some(state))`：窓に既定の入力コンテキストを付け（`ImmAssociateContextEx(IACE_DEFAULT)`）、候補の窓の位置を `ImmSetCandidateWindow(CFS_EXCLUDE)` でカーソルの矩形にし、`ImmSetCompositionWindow(CFS_POINT)` も合わせる。日本語の IME はこれで候補が文字の下に出る。
- `set_ime(None)`：未確定の文字列があれば `ImmNotifyIME(NI_COMPOSITIONSTR, CPS_CANCEL)` で消し、入力コンテキストを外す（`ImmAssociateContextEx(NULL, 0)`）。テキスト入力以外で IME が文字を取らない。
- `surrounding` を使う再変換（`IMR_RECONVERTSTRING`）は今後。

## カーソル

`WM_SETCURSOR` で、クライアント領域（`HTCLIENT`）なら core の指定した形を `SetCursor` する。それ以外は `DefWindowProc` に任せ、リサイズの縁の形などはシステムのものになる。

| `CursorIcon` | Windows |
|---|---|
| `Default`、`Copy` | `IDC_ARROW`（コピーの形は無い） |
| `Pointer`、`Grab` | `IDC_HAND` |
| `Text` | `IDC_IBEAM` |
| `NotAllowed` | `IDC_NO` |
| `Grabbing`、`Move` | `IDC_SIZEALL` |
| `EwResize`、`NsResize` | `IDC_SIZEWE`、`IDC_SIZENS` |
| `Crosshair` | `IDC_CROSS` |
| `None` | `SetCursor(NULL)` |

## ポップアップ

Windows にもポップアップの役割は無い。X11 と同じく、自分で置き場所を決めるトップレベルのウィンドウで作る。

| 種類 | Windows | 外をクリックすると | 入力 |
|---|---|---|---|
| `Menu` | `WS_POPUP`、`WS_EX_NOACTIVATE \| WS_EX_TOOLWINDOW \| WS_EX_NOREDIRECTIONBITMAP`。所有者はメインウィンドウ（入れ子なら親のポップアップ） | 閉じる | 受ける |
| `Tooltip` | 同じ窓で、`WM_NCHITTEST` に `HTTRANSPARENT` を返す（クリックが下の自分のウィンドウに抜ける） | 閉じない | 素通り |
| `Bubble` | `Menu` と同じ窓。ウィンドウが動いたら置き直す | 閉じない | 受ける |

- ポップアップはアクティブにならない（`WS_EX_NOACTIVATE`、`WM_MOUSEACTIVATE` に `MA_NOACTIVATE`）。キーボードはメインウィンドウに残り、core はそこからメニューを操作する。Chromium のメニューも同じ形である。
- **置き場所：** X11 と同じく `place_popup` で決める。範囲はアンカーの中心を含むモニターの作業領域（`MonitorFromPoint` と `GetMonitorInfo` の `rcWork`）。ウィンドウ座標との変換は `ClientToScreen`。往復を待たないので、`PopupConfigured` はすぐに出る。
- **閉じる経路：**

| 経路 | 見つけ方 |
|---|---|
| 他のアプリをクリック、Alt+Tab | メインウィンドウの `WM_ACTIVATE(WA_INACTIVE)`。`Menu` を閉じる |
| タイトルバーなど自分の窓の枠を押す | `WM_NCLBUTTONDOWN`。`Menu` を閉じる |
| 自分のウィンドウの中を押す、Escape、別のメニュー | Wayland と同じ（core が閉じる） |
| メインウィンドウが動いた | `WM_MOVE`。`Menu` と `Tooltip` を閉じ、`Bubble` は置き直す |

## クリップボード

- `CF_UNICODETEXT` で読み書きする。書くときは `\n` を `\r\n` にし、読むときは戻す。
- 開くのは `OpenClipboard(hwnd)`。他のアプリが開いていれば、少し待って数回やり直す。
- Windows に primary selection は無い。`ClipboardKind::Primary` の書き込みは捨て、読み出しには答えない（macOS と同じ）。

## 複数ウィンドウ

- 1 つのスレッドのセッションが、そのスレッドで作った全部のウィンドウを持つ。`spawn_window` は新しいスレッドで `Session::join` し、そのスレッドでウィンドウを作って回す。
- 起こす合図（イベントオブジェクト）と垂直同期のスレッドはセッションごと。
- `ParentLink` は `HWND`。別のスレッドのウィンドウの所有者にもなれる（上の「入力キューがつながる」に注意）。

## ドラッグ＆ドロップ

最初の段階では OS との DnD は持たない（`supports_drag` が偽）。アプリの中の DnD は core だけで動く。

今後は OLE の DnD にする。

- 受ける側：`RegisterDragDrop` で `IDropTarget` を登録する。`DragEnter` / `DragOver` / `DragLeave` / `Drop` を `DragEntered` などにし、`IDataObject` の形式（`CF_UNICODETEXT` は `text/plain;charset=utf-8`、`CF_HDROP` は `text/uri-list`）を MIME の名前に直す。
- 渡す側：`DoDragDrop` はモーダルなループなので、ライブリサイズと同じくポンプで描き続ける。

## Chromium との比較

| | Chromium | AntiWebUI |
|---|---|---|
| ウィンドウ | 自前の `HWND`（`HWNDMessageHandler`） | 自前の `HWND` |
| 描画 | DirectComposition と DXGI のスワップチェーン、`Present1` の dirty rects | DirectComposition と DXGI のスワップチェーン（wgpu）。dirty rects は今後 |
| 垂直同期 | `D3DKMTWaitForVerticalBlankEvent` のスレッド | `IDXGIOutput::WaitForVBlank` のスレッド |
| IME | TSF（以前は IMM32） | IMM32 |
| ポップアップ | アクティブにならないトップレベルの窓 | 同じ |
| タッチパッド | DirectManipulation | DirectManipulation（無ければ `WM_MOUSEWHEEL`） |

## 段階

| 段階 | 内容 | 状態 |
|---|---|---|
| W1 | ウィンドウ、イベントループ、ライブリサイズ、DPI、マウス、キーボード、カーソル、wgpu の Surface、垂直同期、スタック | 2026-11-08 に作った |
| W2 | IME（IMM32）、クリップボード、ポップアップ、フルスクリーン、最大化、CSD、複数ウィンドウ | 2026-11-08 に作った |
| W3 | タッチパッド（DirectManipulation：スクロール、慣性、ピンチ） | 2026-11-08 に作った |
| W4 | OLE の DnD、ファイルのドロップ | 今後 |
| W5 | 自前のスワップチェーンと `Present1` の dirty rects | 今後 |
| W6 | TSF、再変換 | 今後 |

### 確かめたこと（2026-11-08、Windows 11、175% の画面）

`cargo test -p anti-webui-app --test windows`（23 項目、すべて通る）：

- ウィンドウが、最初のフレームまで隠れたあと出ている（`DWMWA_CLOAKED`）。
- クライアント領域が論理サイズ × スケール（400×300 → 700×525）で、core がその論理サイズでレイアウトする。
- クリック、右クリック、ホイール（下にスクロール）。
- ウィンドウが DirectManipulation のビューポートを持つ（タッチパッドの指そのものは合成できない）。
- キーが `TranslateMessage` の文字を打つ、Backspace、キーに続かない文字（`WM_CHAR`）、Ctrl+A。
- ポップアップが自分のウィンドウになり、その上のクリックが届き、外のクリックで閉じる。
- 画面の画素（`GetPixel`）で、ポップアップが映り、影の外側に後ろのウィンドウが透けている。Vulkan を選ばせると、影の外側が黒（`(0, 0, 0)`）になってこの項目が落ちる。
- リサイズでレイアウトし直す（175% で割り切れない大きさは、近い整数の物理ピクセルになる）。
- 最大化、フルスクリーン（画面を覆う）と、そこからの戻り。
- `open_window` と `spawn_window`（別のスレッドのウィンドウ）が開き、閉じる。
- 閉じるボタン（`WM_CLOSE`）が閉じる前のハンドラを通る。

`widgets`、`showcase`、`multi_window`、`svg` の例が、175% の画面で描かれることも見た（`ANTIWEBUI_DUMP_FRAME`）。

### まだ確かめていないこと

- 実際の IME（Microsoft IME など）での未確定文字列と候補の窓の位置。合成のメッセージでは、IMM32 の入力コンテキストを動かせない。
- 手での操作：ライブリサイズ中の描画（ポンプ）、`Client` の飾りの縁でのリサイズ、モニターをまたいだときの `WM_DPICHANGED`、ポップアップの影の透け方、他のアプリのクリックでメニューが閉じること。
- 垂直同期のスレッドが、リフレッシュレートの違う複数のモニターでどう振る舞うか。
- 実際のタッチパッドでの DirectManipulation：スクロールの向き、1 回のジェスチャーで長く動かしたとき、慣性、ピンチ。

## やらないこと

- 排他的なフルスクリーン。
- GDI や Direct2D での描画。
- Windows 7 と 8（Per-Monitor v2 は Windows 10 1703 から）。

## 確かめ方

```sh
cargo test -p anti-webui-app --test windows
cargo test -p anti-webui-windows
cargo run -p anti-webui --example showcase
cargo run -p anti-webui --example widgets
cargo run -p anti-webui --example multi_window
# wgpu の Surface でダメージの出方を見る
ANTIWEBUI_DEBUG_DAMAGE=1 cargo run -p anti-webui --example widgets
```

手で確かめること：125% と 150% の画面で枠線がにじまないこと、モニターをまたいだときに大きさが合うこと、ライブリサイズ中に絵がついてくること、日本語 IME（Microsoft IME）で未確定文字列がインラインに出て候補の窓が文字の下に出ること、メニューが他のアプリのクリックで閉じること。
