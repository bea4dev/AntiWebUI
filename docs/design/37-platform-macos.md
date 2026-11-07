# 37. macOS バックエンド

Wayland と X11 でできていることを、macOS でも同じ書き味で動かす。描く仕組み（core、レンダラ、`Runner`、`Presenter`、`Pacing`）はそのまま使い、ウィンドウ、入力、提示、ポップアップ、クリップボード、IME を AppKit につなぐ。

## 方針

- **AppKit を直接使う。** `winit` などのウィンドウのライブラリは使わない。Wayland と X11 と同じく、`Session`、`Ctx`、ハンドラの形をそろえ、`app` の `Host` と `Platform` に一行ずつ足すだけでつながるようにする。
  - Objective-C の呼び出しは `objc2` と `objc2-app-kit`、`objc2-foundation`、`objc2-quartz-core` を使う。wgpu の Metal も同じ `objc2` の上にあるので、増える依存は `objc2-app-kit` と `dispatch2` だけである。
  - Chromium も `ui/views` の下は自前の `NSWindow` と `NSView`（`BridgedContentView`）で、AppKit のコントロールは使わない。
- **イベントループは今の形のまま。** `NSApp.run()` に制御を渡さず、`dispatch`（溜まった入力を渡して描く）と `wait`（締め切りまで眠る）を `Thread::run` が回す。
- **AppKit はメインスレッドだけ。** `App::run` はメインスレッドで呼ぶ。そうでなければエラーを返す。
- **描画は Metal で `IOSurface` に。** 自前の `IOSurface` を回して変わったところだけ描き、層の `contents` にする（[05](05-rendering.md) の macOS の行）。wgpu の Surface（`CAMetalLayer`）はフォールバックとして残す（後述）。
- **キーの決まりは macOS に合わせる。** Command がショートカットの修飾キーで、Option は文字を打つ修飾キーと単語単位の移動である。これは core と部品の側で `Modifiers` のメソッドを通して切り替える（後述）。

## バックエンドの選び方

macOS では `macos` だけを使う。`ANTIWEBUI_BACKEND` は Linux の 2 つを選ぶためのもので、macOS では `macos` 以外を指定するとエラーになる。Cargo の機能 `macos`（既定で有効）で外せる。`wayland` と `x11` のクレートは Linux でだけビルドされるので、macOS の `cargo check --workspace` も通る。

## app の作り替え

[35](35-platform-x11.md) で `Host` に分けたので、足すのは次だけである。

| 場所 | 変更 |
|---|---|
| `host.rs` | `host_for!(anti_webui_macos::Ctx<'_>)` |
| `runner.rs` | `handler_for!`、`platform_for!`、`Backend::MacOS`、`Link::MacOS` |
| `runner.rs` の `Platform` | `own_threads()` と `idle()` を足す。macOS は `idle` でメインスレッドのイベントを回す。終わるときに spawn したスレッドを待つ間も、それらの頼みごと（窓を閉じるなど）をメインスレッドがこなすためである（後述） |
| `present.rs` | 自前のバッファの presenter を OS で分ける：`present/dmabuf.rs`（Linux）、`present/iosurface.rs`（macOS）、ほかは値を持たない型。`Presenter` の本体は OS で分けない |
| `host.rs` | `raw_handles` は `Option` を返す。spawn した窓のスレッドでは AppKit のビューに触れないので `None` になり、presenter は GPU の Surface を作らない。macOS だけ `create_iosurface_buffer` を足す |
| `pacing.rs` | 時計を `clock.rs` に分ける。Linux は `CLOCK_MONOTONIC`、macOS は `CLOCK_UPTIME_RAW`（`Instant` と `CACurrentMediaTime` と同じ時計）。GPU のフェンス（sync file）は Linux のみ |
| `wgpu` の `configure_window` | Metal は `PreMultiplied` を持たず、`Opaque` だと層を不透明にしてしまう。macOS では `PostMultiplied` を選ぶ。Core Animation はどちらでもプリマルチプライドとして合成するので、絵は変わらず、層が透明を持てるようになるだけである |

`Host` のメソッドの macOS での中身は次の通り。

| `Host` のメソッド | macOS |
|---|---|
| `frame_pending`、`last_frame_callback` | 最後の提示のあと、`CADisplayLink` がまだ来ていない |
| `surfaces_ready` | 提示のあとの `CADisplayLink` が来た |
| `take_presentations`、`presentation_clock` | 提示のあとの次の `CADisplayLink` の `timestamp` を、画面に出た時刻とみなす。周期は `targetTimestamp - timestamp`。時計は `CLOCK_UPTIME_RAW` |
| `dmabuf_feedback` ほか dmabuf と同期 | 無い（`None`、何もしない） |
| `create_iosurface_buffer`、`destroy_buffer` | `IOSurface` に `BufferId` を付けて持つ |
| `present_buffer` | 面の層の `contents` を `IOSurface` にする。変わった矩形は渡す先が無いので捨てる。窓なら提示を記録し、`CADisplayLink` を動かす |
| `take_buffer_events` | 何も来ない。空いたかどうかは presenter が `IOSurfaceIsInUse` で見る |
| `set_opaque` | 自前のバッファなら、それを出す層の `opaque`。フォールバックなら `CAMetalLayer.opaque` |
| `prepare_commit`、`committed` | フォールバック用。`committed` で提示を記録し、`CADisplayLink` を動かす |
| `take_exposed` | 隠れていたウィンドウが見えるようになった（`occlusionState`） |
| `raw_handles` | `AppKitDisplayHandle` と、ウィンドウかポップアップの `NSView`。spawn した窓とドラッグの絵は `None` |

## ウィンドウ

- **`NSWindow`：**
  - スタイルは `Titled | Closable | Miniaturizable | Resizable`。
  - `releasedWhenClosed` は `false`。閉じるのは `Session::close` だけで、そこで `orderOut` してから捨てる。
  - 最初の大きさは論理ピクセルのまま `contentRect` に渡す。1 枚目は画面の中央、2 枚目からは前の窓から少しずらす（`cascadeTopLeftFromPoint`）。
  - `tabbingMode` は `Disallowed`。macOS が勝手にタブにまとめないようにする。
- **ビュー：** `NSView` のサブクラス `AntiWebUIView` を 1 つ作り、ウィンドウとポップアップの両方に使う。
  - `isFlipped` を `true` にして、座標の原点を左上にする。core と同じ向きになる。
  - `makeBackingLayer` で `CAMetalLayer` を返す。フォールバックのとき、wgpu（`raw-window-metal`）はこれをそのまま使う。
  - 自前のバッファは、その上に重ねた `CALayer`（中身の層）に出す。左上を原点に、バッファの大きさ（点）に置く。ビューが `isFlipped` なので、AppKit は親の層を `geometryFlipped` にしていて、原点は左上になる。
  - どちらの層も `contentsGravity` を `topLeft` にする。リサイズの途中で古い絵が伸び縮みしない。
- **窓の飾り（[36](36-multi-window.md)）：**

  | `DecorationMode` | すること | `Decorations` |
  |---|---|---|
  | `Auto`、`Server` | 普通のタイトルバー | `Server` |
  | `Client` | `FullSizeContentView`、タイトルバーを透明に、題名を隠す、信号のボタン 3 つを隠す | `Client` |

  `Client` でも、縁でのリサイズは OS がする。`title_bar` が `start_window_move` を呼ぶと `performWindowDragWithEvent:` で動かす。
- **ウィンドウ操作：**

  | 操作 | macOS |
  |---|---|
  | 移動 | 最後のマウスの押下の `NSEvent` で `performWindowDragWithEvent:` |
  | リサイズ | 何もしない（縁は OS が受ける） |
  | 最小化 | `miniaturize:` |
  | 最大化 | `zoom:`。`isZoomed` が変わったら `Maximized` で知らせる |
  | フルスクリーン | `toggleFullScreen:`（別のスペースに移る）。`windowDidEnterFullScreen:` と `windowDidExitFullScreen:` で知らせる |
  | WM のメニュー | 無い。何もしない |
- **閉じる：** `windowShouldClose:` で `CloseRequested` を出し、`false` を返す。閉じるかどうかは core が決める（閉じる前のハンドラ）。
- **ダイアログ：** `dialog` と `modal` の窓は親の `addChildWindow:ordered:` で親の上に留める。親は `windowNumber`（`ParentLink`）で探す。シートにはしない。シートは親のタイトルバーの下に貼り付き、大きさも位置も OS が決めるので、ほかの OS と見え方が大きく変わるためである。親の入力は core が止める（[36](36-multi-window.md)）。

## スケールとサイズ

- スケールはウィンドウの `backingScaleFactor`（1 か 2）。`Scale(n)` は `n = round(factor × 120)`。macOS には分数のスケールが無いので、1 論理ピクセルは常に整数の物理ピクセルになる。「見た目の大きさ」を変える設定は、OS が 2 倍で描いた絵を縮めて表示する。これはどのアプリも同じで、こちらでは変えられない。
- バッファの大きさは `round(bounds × factor)`。R3（[01](01-pixel-model.md)）のとおり、これをそのままルートの制約にする。`CAMetalLayer.drawableSize` も wgpu がこの大きさにする。
- 別のスケールの画面に移ると `viewDidChangeBackingProperties` が来る。層の `contentsScale` を合わせ、`Resized` を出す。

## イベントループ

```
loop {
    dispatch: 各ウィンドウの受け箱のイベントを渡す、タイマー、描く
    命令（開く、閉じる）
    wait: nextEventMatchingMask(until: 締め切り) → sendEvent、溜まっている分も sendEvent
}
```

- **受け箱：** ビューとデリゲートのメソッド（`mouseDown:` など）は `WindowEvent` を作って、ウィンドウの受け箱（`Rc<Shared>`）に入れるだけにする。`State` には触らない。
  - AppKit は、こちらの呼び出しの途中で同じ窓のメソッドを呼び返すことがある（`setFrame:` の中の `setFrameSize:` など）。`State` を借りたまま呼び返されても壊れないようにするためである。
  - 受け箱の借用はメソッドの中だけで閉じ、借りたまま AppKit を呼ばない。
- **起こす：** 他のスレッド（画像のデコード、`ui_channel`、`shared_signal`）は `dispatch2` でメインキューに小さな処理を積む。その処理が中身の無い `NSEvent`（`ApplicationDefined`）を `postEvent:atStart:` し、`nextEventMatchingMask` を返らせる。GLFW の `glfwPostEmptyEvent` と同じ方法である。
- **ライブリサイズ：** 窓の縁をドラッグしている間、AppKit は `sendEvent:` の中で自分のループを回し、こちらの `wait` に戻ってこない。そのままでは、つかんでいる間ずっと古い絵が出る。
  - `wait` は `nextEventMatchingMask` と `sendEvent:` を呼んでいる間だけ、自分の `dispatch` を呼ぶ口（ポンプ）をスレッドローカルに置く。
  - `setFrameSize:` と `CADisplayLink` は、ライブリサイズ中ならポンプを呼ぶ。その場で新しい大きさの絵が描かれる。
  - その間は `CAMetalLayer.presentsWithTransaction` を `true` にする。窓の枠と中身が同じトランザクションで変わり、ずれない。
  - ポンプは使っている間は外しておく。入れ子にはならない。
- **メニューとアプリの終了：**
  - 最小限のメインメニュー（アプリ名のメニューに「隠す」「ほかを隠す」「終了」）を作る。
  - 「終了」（⌘Q）と Dock の「終了」は `applicationShouldTerminate:` で取り消し、最初のウィンドウに `CloseRequested` を出す。閉じる前のハンドラを通り、最初の窓が閉じればプログラムが終わる（[36](36-multi-window.md)）。
  - 「編集」メニューは作らない。⌘C などのキーは、メニューを通らずに core に届く。

## 提示

- **自前の `IOSurface`（既定）：** `present/iosurface.rs`。Linux の dmabuf と同じく buffer age で変わったところだけ描く（[05](05-rendering.md)）。
  - `IOSurface`（`BGRA`、4 バイト）を面ごとに 3 枚まで作り、`newTextureWithDescriptor:iosurface:plane:` で Metal のテクスチャにして、wgpu の `create_texture_from_hal` で包む。
  - 描く先は、見せている 1 枚を除き、`IOSurfaceIsInUse` が偽（WindowServer が読み終えた）のもののうち、いちばん新しいフレームを持つもの。無ければ作り、3 枚あれば次のフレームでやり直す。
  - 描いたら、wgpu のキューに空のコマンドバッファを積んで `waitUntilScheduled` を待つ。これで Core Animation の読み出しは描画のあとに並ぶ。Chromium も `IOSurface` を層に渡す前に `waitUntilScheduled` を待つ。
  - `CATransaction`（暗黙のアニメーションを切る）の中で、中身の層の `contents`、`contentsScale`、`frame` を変え、`flush` する。ライブリサイズ中は `flush` しない。AppKit の窓枠のトランザクションと一緒に出るためで、`presentsWithTransaction` と同じ役目である。
  - WindowServer には変わった矩形を渡す口が無い。減るのは GPU の仕事で、合成は層の単位のままである。変わる部分を層に分ける（Chromium の `CALayerTreeCoordinator`）のは今後。
  - 画面との色の扱いは `CAMetalLayer` と同じ。両方で描いた窓をスクリーンショットで比べ、画素が一致した。
- **フォールバック：** `ANTIWEBUI_PRESENT=surface` か、`IOSurface` が使えないとき。wgpu の Surface（`CAMetalLayer`、`Fifo`、`maximumDrawableCount` 3）に全面を描く。中身が変わらなければ描かない。spawn した窓では使えない（`raw_handles` が `None`）。
- **フレームの間隔：** `NSView.displayLinkWithTarget:selector:`（macOS 14 以降）の `CADisplayLink` を使う。ウィンドウのある画面のリフレッシュに合わせて来るので、120 Hz の画面（ProMotion）でも、外部の 60 Hz の画面でも、それぞれの周期になる。
  - 提示したら「待ち」にし、`CADisplayLink` を動かす。次に来たら「待ち」を解き、描き直しが要るならすぐ描く。
  - 入力が来て、待ちでなければ、次のリフレッシュを待たずにすぐ描く。止まっていた画面が最初の入力に早く応える。
  - 描くものも待ちも無ければ `CADisplayLink` を止める。止まっている間は CPU を使わない。
  - `CADisplayLink` が来ないとき（画面の外、macOS 13 以前）のため、待ちは 100 ms で諦める。
- **隠れているとき：** `occlusionState` が見えていない間は描かない。フォールバックの wgpu は隠れたウィンドウで `nextDrawable` を取らずに失敗を返し、そのまま描こうとすると 4 ms ごとの再試行が回り続けるためでもある。見えるようになったら全面を描き直す（`take_exposed`）。
- **画面に出た時刻：** Metal の `presentedTime` は使えない（`IOSurface` には無く、フォールバックでは wgpu の中にある）。代わりに、提示のあとの次の `CADisplayLink` の `timestamp`（直前の vblank）を画面に出た時刻とみなし、周期と合わせて `Pacing` に渡す。これでアニメーションの時刻がリフレッシュの格子に乗る（[10](10-frame-pacing.md)）。GPU が間に合わずに 1 回遅れたときは、それだけずれる。
- **描く側の状態（`Frames`）：** 大きさ、描き直しの要否、提示の記録、`CADisplayLink`、中身の層、`IOSurface` は `frames.rs` の `Frames` に持つ。`NSWindow` とビュー（`State`）とは分け、窓を描くスレッドに置く。見えているか、`CADisplayLink` が来たか、ライブリサイズ中かは `Signals`（`Arc`）で受け取る。

## 入力

| AppKit | `WindowEvent` |
|---|---|
| `mouseMoved:`、`*MouseDragged:`、`mouseEntered:` | `PointerMoved`。座標は `locationInWindow` をビューの座標にし、スケールを掛けた物理ピクセル |
| `mouseExited:` | `PointerLeft`。ただし、同じ窓のポップアップの上に移ったときは出さない |
| `mouseDown:`、`rightMouseDown:`、`otherMouseDown:` と `Up` | `PointerButton`。`buttonNumber` 0、1、2 が `Primary`、`Secondary`、`Middle`。`time` は `timestamp` のミリ秒 |
| `scrollWheel:` | `Scroll`（下の表） |
| `magnifyWithEvent:` | `Pinch`。`scale` は `1 + magnification`。`phase` の `Began`、`Changed`、`Ended`、`Cancelled` |
| `keyDown:`、`keyUp:`、`flagsChanged:` | `Key`、`Ime`（下記） |
| `windowDidBecomeKey:`、`windowDidResignKey:` | `Focused` |

- **スクロール：**

  | `NSEvent` | `source` | `delta` | `phase` |
  |---|---|---|---|
  | `hasPreciseScrollingDeltas`、`phase` あり | `Touchpad` | `-scrollingDelta × scale`（物理ピクセル） | `MayBegin` → `Held`、`Began`、`Changed` → `Moved`、`Ended` → `Lifted { fling: false }`、`Cancelled` → `Lifted { fling: false }` |
  | 同上、`momentumPhase` あり | `Touchpad` | 同上 | `Began`、`Changed` → `Momentum`、`Ended` → `MomentumEnded` |
  | `hasPreciseScrollingDeltas` で phase なし（Magic Mouse の一部など） | `Unknown` | 同上 | `Single` |
  | それ以外（ホイール） | `Wheel` | `-scrollingDelta`（ノッチ。OS の加速が入っている） | `Single` |

  - 慣性は OS が付ける（`fling: false`）。core は自前の慣性を足さない。
  - 符号：macOS の `scrollingDelta` は「内容が動く向き」で、ナチュラルスクロールも反映済みである。core は「正なら内容が上・左に動く」なので、符号を反転する。
- **修飾キー：** `Shift` → `shift`、`Control` → `ctrl`、`Option` → `alt`、`Command` → `logo`。
- **キー：** `keyCode`（物理キーの番号）から名前のあるキー（矢印、F1〜F20、Return、Tab、Delete、Forward Delete、Home、End、Page Up/Down、Escape、Help → Insert）を決める。ほかは `charactersIgnoringModifiers` を `Key::Character` にする（Shift だけは効いた文字になる）。
- **文字：** 名前のあるキーと制御文字、関数キーの私用領域（U+F700〜U+F8FF）は `text` を `None` にする。Command か Control を押しているときも `None`。Option は文字を打つ修飾キーなので、Option で出た文字（`å` など）は `text` に入れる。
- **キーの離し：** AppKit は Command を押している間のキーの `keyUp:` を窓に渡さない。こちらのループが `sendEvent:` を呼ぶので、そのイベントだけは `NSApp` を通さずに、イベントの窓の `sendEvent:` に直接渡す（winit と GLFW も同じことをしている）。
- **Command 付きのキー：** メニューに無いもの（⌘C、⌘A など）は、AppKit がそのまま `keyDown:` に渡す。`performKeyEquivalent:` で横取りする必要はない（合成のイベントで確かめた）。メニューにあるもの（⌘Q、⌘H）はメニューが取る。
- **IME の入力中のキー：** IME が使ったキーは、離しも core に渡さない。

### macOS のキーの決まり（core と部品）

`Modifiers` にメソッドを足し、OS で切り替える。Linux の動きは変わらない。

| メソッド | Linux | macOS | 使うところ |
|---|---|---|---|
| `primary()`（今もある） | `ctrl` | `logo` | ⌘C、⌘V、⌘X、⌘Z、⌘A、⌘S、⌘F など |
| `word()` | `ctrl` | `alt` | 単語単位の移動と削除 |
| `line()` | 無い（常に `false`） | `logo` | ⌘← → で行の端、⌘↑ ↓ で文書の端、⌘⌫ で行の頭まで削除 |
| `types_text()` | `ctrl` も `alt` も無い | `ctrl` も `logo` も無い | 打った文字を入れるか |
| `copies_drag()` | `ctrl` | `alt` | ドラッグでコピーにする |

- テストや、プログラムが作る入力のために、`Modifiers::PRIMARY`、`Modifiers::WORD`、`Modifiers::COPY_DRAG` を置く。それぞれ、その OS でそのメソッドが `true` になる修飾キーだけを持つ。Ctrl を前提にしていた core、部品、PDF、Markdown、コードエディタのテストはこれを使う。
- 部品のショートカットも `primary()` に揃える：コードエディタの ⌘/（コメント）、⌘F、⌘H、⌘⇧[ ]（畳む）、Markdown の ⌘クリック（リンク）、PDF の ⌘Z、⌘F、⌘S、⌘ ＋ − 0、⌘ スクロール（拡大）、⌘クリック（リストの複数選択）、フォームの JavaScript の `event.modifier`（PDF の仕様でも Mac は Command）。
- 複数行の入力欄の ⌘Return（Linux は Ctrl+Enter）は、改行ではなく送信にする。

## IME

`AntiWebUIView` は `NSTextInputClient` を実装する。core の `ImeState`（[04](04-text.md)）が `Some` の間だけ、`keyDown:` で `interpretKeyEvents:` を呼ぶ。

| `NSTextInputClient` | すること |
|---|---|
| `setMarkedText:selectedRange:replacementRange:` | `Ime { preedit: Some((文字, カーソル)) }`。範囲は UTF-16 からバイトに直す |
| `insertText:replacementRange:` | 下記 |
| `unmarkText` | 未確定の文字を確定する |
| `hasMarkedText`、`markedRange`、`selectedRange` | 未確定の文字と、`ImeState` の `cursor` と `anchor`（UTF-16） |
| `firstRectForCharacterRange:actualRange:` | `ImeState.cursor_rect` を画面の座標にする（候補の窓の位置） |
| `attributedSubstringForProposedRange:actualRange:`、`characterIndexForPoint:` | 返さない（`nil`、`NSNotFound`） |
| `doCommandBySelector:` | 記録だけする。キーは `Key` として core に渡す |

1 回の `keyDown:` の間に起きたことを集めて、次のどれかにする。

1. 前か後に未確定の文字がある、または未確定の文字が変わった → IME が使ったキー。`Ime` だけを出し、`Key` は出さない。
2. `insertText:` だけが来た → `Key` を出し、`text` に入った文字を入れる（普通の打鍵、Option の文字）。
3. どちらでもない（`doCommandBySelector:` だけ） → `Key`、`text` は `None`。Backspace、矢印、Return、Tab などは core が処理する。

- `keyDown:` の外で `insertText:` が来たとき（候補をマウスで選んだ）は、`Ime { commit }` を出す。
- Command を押しているキーは `interpretKeyEvents:` を通さない。
- `ImeState` が `None` になったら、`inputContext.discardMarkedText` を呼ぶ。キャレットが動いたら `invalidateCharacterCoordinates` を呼ぶ。

## カーソル

| `CursorIcon` | `NSCursor` |
|---|---|
| `Default` | `arrowCursor` |
| `Pointer` | `pointingHandCursor` |
| `Text` | `IBeamCursor` |
| `NotAllowed` | `operationNotAllowedCursor` |
| `Grab`、`Grabbing` | `openHandCursor`、`closedHandCursor` |
| `Move` | `openHandCursor`（macOS に移動の矢印は無い） |
| `EwResize`、`NsResize` | `resizeLeftRightCursor`、`resizeUpDownCursor` |
| `Crosshair` | `crosshairCursor` |
| `Copy` | `dragCopyCursor` |
| `None` | `NSCursor.hide`（出ていくときに `unhide`） |

ポインタがビューの上にあるときだけ `set` する。追跡範囲の `cursorUpdate:` でも同じものを `set` し、OS がリサイズの縁などで変えたカーソルを戻す。

## ポップアップ

- **窓：** 枠の無い `NSWindow`（`Borderless`）。`opaque` は `false`、背景は透明、`hasShadow` は `false`（影は core が描く）。親の `addChildWindow:ordered:` で親と一緒に動く。
  - 枠の無い窓は key にならないので、キーボードは親の窓に届いたままである。core はメニューのキー操作を親の窓のイベントとして処理する（X11 と同じ）。
  - `Tooltip` は `ignoresMouseEvents` を `true` にする。
  - 影の部分の入力を素通りさせる形は持たない。影は数ピクセルなので、その上のクリックは core が「メニューの外」として扱う。
- **置き方：** X11 と同じ `place`（`anti_webui_core::platform` に移して共有する）。収める範囲は、アンカーのある画面の `visibleFrame`（メニューバーと Dock を除く）。`Bubble` は `position` にそのまま置く。
- **座標：** ポップアップのビューの入力は、ポップアップの原点を足して、親の窓の座標で親の受け箱に入れる（X11 の `origin_of` と同じ）。
- **閉じる：**
  - 親の窓が動いた（`windowDidMove:`）→ `Menu` と `Tooltip` を閉じる。
  - アプリが非アクティブになった、親が key でなくなった → `Menu` を閉じる。
  - 窓の中の押下は core がメニューの外かを判断する。

## クリップボード

- `Clipboard`：`NSPasteboard.generalPasteboard` に `NSPasteboardTypeString` で書く。貼り付けはその場で読んで `Paste` を出す。
- `Primary`：macOS には無い。書くのは捨て、貼り付けには答えない。

## 複数ウィンドウ

- `open_window`：メインスレッドの `Session` にウィンドウを足す。Linux と同じ。
- `spawn_window`：[36](36-multi-window.md) の表のとおり、窓はメインスレッドに置き、UI と描画は spawn したスレッドで動かす（`remote.rs`）。重い窓が他の窓を止めない。

  ```
  spawn したスレッド                          メインスレッド
  Session::join（run loop の source を作る）
  open ── exec_sync ───────────────────────→ State::new（NSWindow、ビュー、中身の層、CADisplayLink）
       ←──────────── Frames（層、CADisplayLink）  HOSTED に登録
  CADisplayLink をこのスレッドの run loop に
  loop {
    dispatch: 受け箱 → Ui、描く（IOSurface → 層）  dispatch: HOSTED の State::take_events → 受け箱 → 起こす
    窓への頼みごと ── exec_async ────────────→ State::set_title など（頼んだ順）
    wait: CFRunLoopRunInMode
  }
  close ── exec_async ─────────────────────→ State::close
  ```

  - **窓への頼みごと：** `Ctx` のメソッドは、窓がこのスレッドにあればその場で、メインスレッドにあれば `FnOnce(&mut State)` をメインキューに積む（`exec_async`）。積んだ順に動く。返事の要るもの（貼り付け、ポップアップの位置）はイベントで返る。
  - **描く：** `IOSurface` への描画と層の `contents` の差し替えは、spawn したスレッドでする。Core Animation の層はどのスレッドからでも変えられる（明示の `CATransaction`）。Firefox もコンポジタのスレッドから層を変えている。メインスレッドが重くても、spawn した窓は描き続ける。
  - **`CADisplayLink`：** メインスレッドがビューから作り（`displayLinkWithTarget:selector:` はビューのメソッド）、spawn したスレッドの run loop に入れる。呼ばれる先はビューではなく、スレッドをまたいでよい小さなオブジェクト（`Ticker`）で、`Signals` に書いて run loop を起こす。
  - **待つ：** `CFRunLoopRunInMode`。起こすのは、何もしない `CFRunLoopSource` を signal して `CFRunLoopWakeUp`。signal は待っていない間にしても消えないので、起こし損ねない。
  - **ポップアップ：** 中身の層は spawn したスレッドで作り、`show_popup` と一緒にメインスレッドに渡して、ポップアップのビューの層に重ねる。そのフレームからすぐ描ける。
  - **入力：** AppKit のイベントはメインスレッドに来る。メインスレッドの `dispatch` が受け箱から渡す。メインスレッドが重い間は、spawn した窓への入力も遅れる（描画は止まらない）。
  - **開く：** 窓を作るのは `exec_sync` で待つ。親の `windowNumber`（`ParentLink`）をすぐ返すためである。メインスレッドが spawn したスレッドを待つのはプログラムの終わり（`end_threads`）だけで、そこでは `Platform::idle` でイベントを回しながら待つので、詰まらない。
  - **できないこと：** ライブリサイズの間、spawn した窓の中身は窓枠と同じトランザクションに乗らない（少しずれうる）。wgpu の Surface のフォールバックは使えない。

## ドラッグ＆ドロップ

`dnd.rs`。ウィンドウのビューが `NSDraggingDestination` と `NSDraggingSource` を兼ねる（[23](23-drag-and-drop.md)）。自分の窓の上の自分のドラッグも、外から来たものと同じ道を通る。

- **MIME と pasteboard の型：**

  | MIME | pasteboard |
  |---|---|
  | `text/plain;charset=utf-8`、`text/plain`、`UTF8_STRING` | `public.utf8-plain-text` |
  | `text/uri-list` | `public.file-url`。1 つのファイルに 1 つの `NSPasteboardItem`。Finder のファイル参照の URL（`file:///.file/id=…`）は `filePathURL` でパスの URL にする |
  | `text/html`、`image/png` | `public.html`、`public.png` |
  | ほか | MIME の文字列をそのまま型にする（自分の窓どうしで通じる） |

- **外から：** `registerForDraggedTypes:` で文字列、ファイル、HTML、PNG を受ける。

  | AppKit | すること |
  |---|---|
  | `draggingEntered:` | `DragEntered`（位置、MIME、修飾キー）。`NSDragOperation::None` を返す |
  | `draggingUpdated:`（周期的にも来る） | 動いたか修飾キーが変わったら `DragMoved`。core の `AcceptDrag` の答えと、元が許す操作（`draggingSourceOperationMask`）から操作を返す。答えは 1 回遅れて効く |
  | `draggingExited:` | `DragLeft` |
  | `performDragOperation:` | 受けると言っていれば `DragDropped` を出して `YES`。中身は `ReceiveDrop` のときに pasteboard から読んで `DropData` にする |

- **外へ：** core の `StartDrag` は `Frames` に置き、その回のループの終わりに `beginDraggingSessionWithItems:event:source:` を呼ぶ。そのフレームで描いたドラッグの絵（`SurfaceId::DragIcon` の `IOSurface`）を `NSBitmapImageRep` に写し、`NSImage` にして渡す。始めの `NSEvent` は最後のマウスの押下か移動。ボタンがもう離れていれば始めずに `DragSourceEnded(Cancelled)`。
  - 許す操作：`Move` → Move と Generic、`Copy` → Copy、`Both` → 全部。
  - `draggingSession:endedAtPoint:operation:` → `DragSourceEnded`。Copy なら `Copied`、Move、Generic、Delete なら `Moved`、なければ `Cancelled`。
  - ドラッグの間は AppKit のループが回る。`draggingSession:movedToPoint:`、`draggingUpdated:`、`CADisplayLink` からポンプ（ライブリサイズと同じ）を呼び、core に答えさせる。
  - core の `CancelDrag`：AppKit のセッションには取り消す口が無いので、すぐ `DragSourceEnded(Cancelled)` を出し、終わったときの知らせは捨てる。Esc は AppKit が処理する。

## Chromium との比較

| | Chromium（`ui/views` の macOS） | AntiWebUI |
|---|---|---|
| ウィンドウ | `NativeWidgetNSWindowBridge`、自前の `NSWindow` と `NSView` | 自前の `NSWindow` と `NSView` |
| 描画 | GPU プロセスで描き、`IOSurface` を `CALayer` に（`CALayerTreeCoordinator`） | 同じプロセスで `IOSurface` に Metal で描き、`CALayer` に。層は面ごとに 1 枚 |
| フレームの間隔 | `CVDisplayLink`（新しい版は `CADisplayLink`） | `CADisplayLink` |
| IME | `RenderWidgetHostViewCocoa` が `NSTextInputClient` | ビューが `NSTextInputClient` |
| ポップアップ | 子の `NSWindow` | 子の `NSWindow` |

## 段階

| 段階 | 中身 | 状態 |
|---|---|---|
| M0 | app の作り替え（`present` と `pacing` の OS での分け方、`own_threads`）、`Modifiers` のメソッド、`place_popup` を core に | 2026-11-07 に作った |
| M1 | ウィンドウ、`CAMetalLayer` への描画、スケール、リサイズ（ライブリサイズを含む）、`CADisplayLink`、閉じる、メニュー | 2026-11-07 に作った |
| M2 | マウス、スクロール、ピンチ、キー、IME、カーソル、クリップボード、フルスクリーン、最小化、最大化、移動、CSD | 2026-11-07 に作った |
| M3 | ポップアップ（`Menu`、`Tooltip`、`Bubble`）、ダイアログ、`open_window` | 2026-11-07 に作った |
| M4 | OS との DnD、`spawn_window` を別のスレッドで描く、`IOSurface` での部分描画 | 2026-11-07 に作った |

### 確かめたこと（2026-11-07、M1 Pro、macOS 26、内蔵の Retina 画面）

- **単体テスト（`cargo test -p anti-webui-macos`）：** 10 件。
- **端から端まで（`cargo test -p anti-webui-app --test macos`）：** 本物の窓に、プログラムが作った `NSEvent` を自分のキューに post する。ユーザーの入力と同じく `sendEvent:` から入り、アクセシビリティの許可は要らない。IME は `NSTextInputClient` のメソッドを、外からのドラッグは `NSDraggingDestination` のメソッドを、作り物の `NSDraggingInfo` で直接呼ぶ。次の 20 件が通る。
  - 窓が出す `IOSurface` が窓の画素と同じ大きさ（2 倍）、クリック、Control クリックが副ボタン。
  - IME の確定、Delete、未確定の文字は値に入らない、候補の窓の位置がキャレット、確定、⌘A で全選択。
  - ポップアップが別の窓になる、その上のクリック、外のクリックで閉じる。
  - 外からの文字のドロップを受ける（操作が返る、`YES`、値が届く）、ファイルのドロップがパスで届く、受け手の無い所では `None`。
  - リサイズでレイアウトと `IOSurface` が追従、閉じるボタンが閉じる前のハンドラを通る。
- **spawn した窓（`cargo test -p anti-webui-app --test macos_spawn`）：** 次の 10 件が通る。
  - 開く、`IOSurface` を窓の画素の大きさで出す、クリックが届く、UI がメインスレッドでないスレッドで動く。
  - メインスレッドを 500 ms 止めても、spawn した窓のタイマーは回り続ける。spawn した窓のスレッドを 600 ms 止めても、メインの窓はクリックに応える。
  - ポップアップが別の窓になり、その上のクリックが届く。リサイズに追従する。メインの窓を閉じると、spawn したスレッドも終わる（詰まらない）。
  - `CADisplayLink` が spawn したスレッドの run loop で来ることも、ログで確かめた（提示のたびに 1 回）。
- **部分描画（`widgets`、`ANTIWEBUI_DEBUG_DAMAGE=1`）：** キャレットの点滅で、窓の 0.4% だけを描き直す（buffer age 2〜4、バッファ 3 枚）。
- **画面の見え方：** `screencapture` で撮ると、`IOSurface` の窓は上下も色も正しい。`ANTIWEBUI_PRESENT=surface`（`CAMetalLayer`）と窓の中の画素が全部一致する。
- **フレームの間隔（`bench_pacing --seconds 4 --rows 2000`、release）：** `IOSurface` で 120 Hz（8.333 ms）、452 フレームで飛んだリフレッシュは 0 回。CPU の p50 は 1.9 ms（`waitUntilScheduled` を含む）。`CAMetalLayer` では 457 フレームで 0 回、1.0 ms。
- **止まっているとき：** `widgets` を開いたまま 5 秒、CPU 時間が増えない（`CADisplayLink` が止まる）。
- **起動（`widgets`、release、2026-11-08）：** `App::run` から最初の提示まで約 305 ms。内訳と、手が出せるかどうか：
  - `NSApplication::sharedApplication` で約 40 ms、`finishLaunching` で約 10 ms。AppKit の初期化で、こちらでは縮められない。
  - `NSWindow` を作るのに約 40 ms。
  - 最初の `makeKeyAndOrderFront` で約 150 ms。窓の開くアニメーションが作るディスプレイリンクで、SkyLight が画面のモードを全部調べる（`SLSIsDisplayModeVRR`）。プロセスで 1 度だけで、アニメーションを切ると、こちらの `CADisplayLink` を作るところへ移るだけ。別スレッドの `NSScreen` のディスプレイリンクで先に済ませると約 55 ms 縮むが、AppKit がメインスレッドの外で保証しないことなので、やらない。CoreVideo、CoreGraphics、`CAMetalDisplayLink` では先に済まない。
  - システムのフォントを調べるのに 35〜45 ms。スレッドごとに調べ直していたので、`App::run` の初めに別スレッドで 1 度だけ調べ、各スレッドはその複製を使う（`text::prepare_system_fonts`）。これで約 35 ms 縮み、spawn した窓のスレッドでも調べ直さない。
  - GPU と描き手で約 10 ms。最初のレイアウトと描画、提示で約 50 ms。
  - ビルドしたての 1 回目だけ、`main` の前に約 1 秒かかる。macOS が新しいバイナリを調べる分で、2 回目からは約 12 ms。
- **Linux：** 手を入れたクレート（core、wgpu、app、wayland、x11）を `x86_64-unknown-linux-gnu` で `cargo check` して、警告なし。実機での確認はまだ。

### まだ確かめていないこと

- 手で：ライブリサイズ（縁をドラッグ。`IOSurface` でも窓枠とずれないか）、実際のキーボードでの日本語入力、トラックパッドのスクロールと慣性、ピンチ、⌘Q、フルスクリーン、Retina と 1 倍の画面の間の移動、`DecorationMode::Client`。
- 外へのドラッグ（Finder やテキストエディットへ文字やファイルを運ぶ、絵がポインタについてくる、`DragSourceEnded` の結果）。本物のマウスが要るので、合成のイベントでは試せない。
- spawn した窓での外へのドラッグ、`start_move`（メインスレッドに渡る間にボタンが離れていないか）。
- `app` の `render.rs` の `text_scaled_up_is_as_sharp_as_bigger_text` は macOS では今回の変更の前から落ちる（ヘッドレスの描画で、フォントの違いと見られる）。

## やらないこと

- **シート、ネイティブのメニュー、ネイティブのコントロール：** 見え方を OS ごとに変えないため。
- **macOS 13 以前の `CADisplayLink`：** 無ければ 100 ms の待ちの上限で動く（描くのは遅れず、提示の間隔が粗くなるだけ）。

## 確かめ方

- **単体テスト（`cargo test -p anti-webui-macos`）：** キーの番号と文字、修飾キー、UTF-16 とバイトの範囲の変換、スクロールの向きと段階。
- **端から端まで（`cargo test -p anti-webui-app --test macos`、`--test macos_spawn`）：** 上の「確かめたこと」。窓が数秒開く。テストの道具は `anti_webui_macos::testing`（doc(hidden)）。
- **画面に出たもの：** `screencapture -x` で撮れる（画面収録の許可があれば）。
- **手で：**
  - `cargo run -p anti-webui --example showcase`、`widgets`、`multi_window`。
  - Retina の画面と外部の 1 倍の画面の間で窓を動かす（スケールが変わり、にじまない）。
  - 縁をつかんでリサイズ（中身がついてくる）。
  - 日本語入力（未確定の文字がインラインに出る、候補の窓がキャレットの下に出る）。
  - トラックパッドのスクロールと慣性、ピンチ。
  - プルダウンを開いて窓を動かす（閉じる）、ほかのアプリをクリック（閉じる）。
  - ⌘Q で閉じる前のハンドラが呼ばれる。
  - `widgets` の「Dragging」や PDF の文字を Finder、テキストエディットへ運ぶ。Finder のファイルや Safari の文字を窓に落とす。
  - `multi_window` で spawn した窓を開き、両方の窓が別々に動く。
- **`ANTIWEBUI_DUMP_FRAME`：** 提示した絵をファイルに書く。Linux と同じく使える。
