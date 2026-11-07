# 36. 複数のウィンドウと窓の飾り

サブウィンドウ（親を持つウィンドウ）を開けるようにする。ウィンドウごとに、親と同じ UI スレッドで動かすか、専用のスレッドで動かすかを選べる。あわせて、窓の飾り（SSD と CSD）の取り決め、閉じるボタンが押されたときのハンドラ、CSD 用のウィンドウ操作（移動、リサイズ、最小化、最大化）を用意する。

## API

```rust
// 同じスレッド。シグナルをそのまま共有できる
let settings = open_window(
    WindowOptions::new().title("設定").size(480, 360),
    move || settings_view(theme),            // 'static
);

// 専用のスレッド。重い処理が他のウィンドウを止めない
let editor = spawn_window(
    WindowOptions::new().title("file.md"),
    move || editor_view(path),               // Send + 'static
);

// 親の上に留まるダイアログ（WM はダイアログとして飾る）
open_window(WindowOptions::new().title("検索").dialog(true), find_view);

// 親の入力を止めるダイアログ（dialog も兼ねる）
open_window(WindowOptions::new().title("保存しますか？").modal(true), confirm_view);

// 開いた窓と一緒には閉じない
open_window(WindowOptions::new().top_level(), other_view);

// ハンドル（Send + Sync + Clone）。どのスレッドからでも使える
settings.set_title("設定 *");
settings.request_close();                   // 閉じる前のハンドラを通す
settings.close();                           // すぐ閉じる
settings.is_open();

// 今のウィンドウ
current_window();
close_window();                             // すぐ閉じる
request_close_window();                     // 閉じるボタンと同じ
set_window_title("…");

// 閉じるボタン（SSD でも CSD でも）、Alt+F4、WM の「閉じる」
on_close_request(move |req| {
    if dirty.get() {
        req.prevent();
        ask_to_save.set(true);
    }
});

// 窓の飾り
App::new().decorations(DecorationMode::Auto)   // Auto（既定）| Server | Client
window_decorations();                          // Decorations::Server | Client。読むと追跡される
window_maximized(); window_active();           // 読むと追跡される
start_window_move();                           // ポインタを押している間に呼ぶ
start_window_resize(ResizeEdge::BottomRight);
minimize_window();
set_window_maximized(true); toggle_window_maximized();
show_window_menu();                            // ポインタの位置に WM のメニュー

// スレッドをまたぐシグナル。中で Mutex を使うが、ユーザーには見せない
let progress = shared_signal(0.0f32);
spawn_window(opts, { let p = progress.clone(); move || worker_view(p) });
text(move || format!("{:.0}%", progress.get() * 100.0))  // どのスレッドでも、読むと追跡される
```

- **ウィンドウが閉じたことを知る：** ルートの中の `on_cleanup` が、ウィンドウが閉じたときに走る。同じスレッドならシグナルを、別のスレッドなら `shared_signal` か `ui_channel` を使って知らせる。
- **CSD のタイトルバー：** `anti-webui-widgets` の `title_bar` を使う。CSD のときだけ現れる（後述）。

## スレッドの選び方

| | `open_window`（同じスレッド） | `spawn_window`（専用のスレッド） |
|---|---|---|
| シグナル | 開いたウィンドウとそのまま共有できる | 共有できない。`shared_signal` か `ui_channel` を使う |
| ルートのクロージャ | `'static` | `Send + 'static`。シグナルを持ち込むとコンパイルエラーになる |
| 重い処理 | 同じスレッドのウィンドウがまとめて止まる | 他のウィンドウは止まらない |
| GPU | スレッドの GPU デバイスを共有する | スレッドごとに GPU デバイスを作る（後述） |
| 向いているもの | 設定、ダイアログ、インスペクタ、発表者用の画面 | 別のファイルを開く窓、重い文書や動画の窓 |

- **Mutex：** ウィンドウの中のコードは今まで通りシングルスレッドで、Mutex は要らない（[05](05-rendering.md) の UI スレッドの決まりはウィンドウの中の話）。スレッドをまたぐところでは、Mutex を直接扱うより楽になるように `shared_signal` を用意する。中は `Mutex` と各スレッドへの通知で、読んだスレッドのエフェクトが、そのスレッドで走り直す。
- **Chromium との比較：** Chromium はブラウザの UI（全ウィンドウ）を 1 つのスレッドで動かし、ページの中身をタブごとのプロセスに分ける。`spawn_window` はプロセスではなくスレッドで分けるので、それより軽い。

### スレッドごとのもの

| もの | 同じスレッドの窓 | spawn したスレッド |
|---|---|---|
| `settings()` / `update_settings` | 共有 | 開いた時点の値を写す。以後どのスレッドで変えても全 UI スレッドに配る |
| `register_font` で登録したフォント | 共有 | 登録をスレッドの始めに繰り返す（データは `Send + Sync` で受け取っている） |
| 動画のデコーダー | 共有 | スレッドの始めに入れる |
| 標準の部品のテーマ、MD3E の配色 | 共有 | スレッドごと。spawn したルートで `set_theme` などをもう一度呼ぶ |
| 画像のキャッシュ | ウィンドウごと（今までの文書ごと） | 同じ |

## 中の作り：core

### ウィンドウごとの状態

今の core は、文書（`tree.rs` の `DOC`）などをスレッドローカルに 1 つずつ持つ。これを「いまのウィンドウ」の分として扱い、ウィンドウを切り替えるときに入れ替える。

- **入れ替えるもの：** 文書、`request_animation_frame` のコールバック、`set_timeout` のタイマー、アクセシビリティの監視、`window_scale` と `window_fullscreen` のシグナル、ウィンドウの情報（閉じる前のハンドラ、飾り、最大化、開いているモーダルの子の数など）。
- **共有するもの：** リアクティブのランタイム（シグナルを窓の間で共有できる）、文字の仕組み（フォント）、設定、`ui_channel` の受け口、ドラッグ中の値。
- **入れ替え方：** 使っていないウィンドウの分は、ウィンドウの番号の表にしまっておき、切り替えるときに `mem::swap` で入れ替える。ウィンドウが 1 枚のアプリでは、切り替えが起きないので何も変わらない。`with_doc` などの普段の道は、今と同じスレッドローカルへの 1 回の借用のまま。

### いつ切り替えるか

| 入口 | 切り替え先 |
|---|---|
| `Ui` のメソッド（イベント、更新、描画、タイマー、画像） | その `Ui` のウィンドウ |
| エフェクトとメモの計算、`on_cleanup` | 作られたときのウィンドウ（下記） |
| `ui_channel` の受け取り | チャネルを作ったときのウィンドウ |
| `NodeRef` のメソッド | ノードを結び付けたときのウィンドウ |

- **リアクティブのノード：** 作られたときのウィンドウを覚える。オーナーがあればオーナーのもの、なければその時のウィンドウ。走らせる間はそのウィンドウにして、終わったら戻す。
- **例：** ウィンドウ A のクリックでシグナルを書き、ウィンドウ B のエフェクトが走り直すときは、B の文書が触られる。
- **ノードの番号：** 文書ごとの `SlotMap` の鍵なので、窓をまたいで使うと別のノードを指しうる。`NodeRef` は結び付けたウィンドウを覚えるので安全に使える。`View` を別のウィンドウに入れるのは誤りで、デバッグビルドでは `set_children` が気付いて止める。

### ウィンドウを開く、閉じる

- **`open_window`：** その場では作らず、スレッドの「開く予定」の列に入れて、ハンドルを返す。app がイベントの処理を終えてから、新しい `Ui` を作ってルートを組み立てる。ハンドラの途中で別の文書を組み立てないためである。
- **`spawn_window`：** スレッドの「spawn の予定」の列に入れる。app がスレッドを立て、そのスレッドで `Ui` を作る。
- **閉じる：** app が `Ui::close` を呼ぶ。ルートのオーナーを捨て（`on_cleanup` が走る）、しまってあったウィンドウの状態も捨てる。
- **子：** 開いたウィンドウ（親）が閉じると、子も閉じる。`WindowOptions::top_level()` で開いたものは、親と一緒には閉じない。
- **プラットフォームに親を伝えるか：** 「一緒に閉じる」とは別に決める。

| | プラットフォームに親を伝える | 開いた窓と一緒に閉じる |
|---|---|---|
| 既定 | しない。普通の窓で、最大化などのボタンが全部そろう（ブラウザや Slack が開く窓と同じ） | する |
| `.dialog(true)` | する。親の上に留まり、WM はダイアログとして飾る（閉じるボタンだけのことが多い） | する |
| `.modal(true)` | する（モーダル） | する |
| `.top_level()` | しない | しない |

  親を伝えた窓に最大化ボタンを出すかは WM の方針で、プロトコルの制限ではない。ShojiWM は出さない。
- **メインウィンドウ：** `App::run` が開いたものが閉じると、全スレッドの全ウィンドウを閉じ、スレッドの終わりを待ってから `run` が戻る。

### ハンドル

`WindowHandle` は `Arc` の中にウィンドウの番号、開いているか（`AtomicBool`）、届け先を持つ。

- **届け先：** 同じスレッドならスレッドローカルの命令の列。別のスレッドなら、そのスレッドの `ui_channel`。
- **spawn した直後：** 届け先のスレッドがまだ始まっていないうちの命令は溜めておき、スレッドが始まってから流す。
- **命令：** 閉じる、閉じる前のハンドラを通して閉じる、タイトル、モーダルの子が開いた／閉じた（親の入力を止めるため。親が別のスレッドでも届く）。
- **順番：** 命令を 1 つ片付けるたびに、それから生まれた命令を残りより先に片付ける。ダイアログが閉じて親の入力が戻ってから、その後に頼まれた「親を閉じる」が届く、という順になる。

### 閉じる前のハンドラ

1. プラットフォームから `CloseRequested` が来る（SSD の閉じるボタン、Alt+F4、WM の「閉じる」）。`request_close_window()` と `WindowHandle::request_close()` も同じ道を通る。
2. `on_close_request` のハンドラを、登録した順に、文書を借りずに呼ぶ。
3. どれかが `prevent()` しなければ、`PlatformRequest::Close` を出す。
4. モーダルの子が開いている間は、親への `CloseRequested` を無視する。

- `close_window()` と `WindowHandle::close()` は、ハンドラを通さずに閉じる。コードが閉じると決めたのだからである。
- ハンドラは、登録したオーナーが捨てられると外れる。

### モーダル

- **プラットフォーム：** Wayland では `xdg_wm_dialog_v1.set_modal`（あれば）、X11 では `_NET_WM_STATE_MODAL` と `_NET_WM_WINDOW_TYPE_DIALOG`。
- **core：** 親の `Ui` は、モーダルの子が開いている間、ボタン、スクロール、ピンチ、キー、IME の入力を捨てる。ポインタの移動は、ホバーを外すためだけに受ける。Windows で親を無効にするのと同じ見え方である。

## 中の作り：app

### スレッドとウィンドウ

```
メインスレッド：Session（接続、起こすためのパイプ）
  ├ ウィンドウ（バックエンドのインスタンス + Runner(Ui, Presenter, Pacing)）
  └ ウィンドウ …（open_window で開いたもの）
spawn したスレッド：Session（Wayland は同じ接続、X11 は自分の接続）
  └ ウィンドウ …
```

1 周の流れは次の通り。

1. **dispatch：** 各ウィンドウのイベント、タイマー、フレームを処理する。命令などで `Ui` に頼んだことは、パイプで起こして `woken` から処理させる（プラットフォームへの頼みごとはハンドラの中でしか出せないため）。
2. **命令：** 開く予定、spawn の予定、閉じる命令、プラットフォームが閉じたウィンドウを処理する。
3. **wait：** 全ウィンドウの締め切りのうち最も早いものまで、接続とパイプを待つ。

- **Runner：** 今の `Runner` がそのままウィンドウ 1 枚分になる。フレームクロックもウィンドウごとで、別のモニターにあれば周期も別に追う。
- **起こすパイプ：** スレッドに 1 本。画像のデコード、`ui_channel`、ハンドルの命令は、どれもこのパイプで起こす。起きたら全ウィンドウの `woken` を呼ぶ。

### GPU

- **同じスレッドの窓：** GPU デバイスと dmabuf の文脈（コマンドプールと Vulkan の関数）を共有する。2 枚目以降のウィンドウは、デバイスを作る時間（数十 ms）がかからない。
- **spawn したスレッド：** 自分のデバイスを作る。dmabuf の文脈は生の `VkQueue` に提出するので、別のスレッドの wgpu の提出と同じキューを使うと同期が取れないためである。
- **レンダラー（グリフのアトラス）：** ウィンドウごとに持つ。共有はあとで、アトラスの大きさを測ってから決める。

## 中の作り：バックエンド

どちらのバックエンドも、今の「ウィンドウ 1 枚分の状態」をそのままインスタンスとして使い、スレッドの `Session` が複数を持つ。ウィンドウ 1 枚の道はほとんど変わらず、確かめてある部分を使い回せる。

### Wayland

- **接続：** プロセスで 1 本。spawn したスレッドも同じ接続を使うので、コンポジタから見て 1 つのクライアントのままで、`set_parent` がスレッドをまたいで効く。
- **キュー：** ウィンドウごとに `EventQueue` を持ち、グローバルもウィンドウごとに bind する。
- **読み方：**
  - 各キューを dispatch してから、1 つのキューで `prepare_read` を取る。
  - そのあと残りのキューをもう一度 dispatch し、何か来ていれば読むのをやめてやり直す。
  - `prepare_read` の後は、他のスレッドが読んでも、こちらが読むか取り消すまで待つ（libwayland の決まり）。なので、取りこぼしはない。
- **入力：** `wl_pointer`、`wl_keyboard`、`text_input`、`wl_data_device` もウィンドウごとに作る。
  - コンポジタは、クライアントのすべての入力オブジェクトにイベントを送る（wlroots、KWin、Mutter、Smithay のどれも同じ）。
  - 各インスタンスは `enter` の surface が自分のものかを見て、`leave` までの入力を受けるか捨てるかを決める。
  - 自分のものかは、`SurfaceId` ではなくオブジェクトの同一性で比べる。どのインスタンスも同じ型のユーザーデータを付けているためである。
- **親（`dialog` と `modal` のとき）：** `xdg_toplevel.set_parent`。モーダルは `xdg_wm_dialog_v1`（staging の xdg-dialog）。
- **閉じる：** ウィンドウのオブジェクトを壊し、そのキューを捨てる。

### X11

- **接続：** ウィンドウごとに自分の接続を開く。ウィンドウの ID はサーバー全体で通じるので、`WM_TRANSIENT_FOR` はどの接続からでも指せる。
- **代わりにかかるもの：** 接続ごとに、XKB のキーマップ、XIM、拡張の問い合わせが 1 回ずつかかる。
- **他のウィンドウとのやり取り：** クリップボードと DnD は、別のアプリと同じ道でやり取りする。同じスレッドでも、イベントの処理を待ってから答えるので詰まらない。
- **親（`dialog` と `modal` のとき）：** `WM_TRANSIENT_FOR`。モーダルなら `_NET_WM_WINDOW_TYPE_DIALOG` と `_NET_WM_STATE_MODAL`。

## 窓の飾り

### 取り決め

| | Wayland | X11 |
|---|---|---|
| `Auto`（既定） | xdg-decoration で `server_side` を頼む。コンポジタの `configure` の答えに従う。xdg-decoration がなければ `Client` | WM に任せる（`Server`） |
| `Server` | `server_side` を頼む。それでも `client_side` が返れば `Client` | WM に任せる |
| `Client` | `client_side` を頼む | `_MOTIF_WM_HINTS` で WM の飾りを外す |
| 知らせ | `zxdg_toplevel_decoration_v1.configure` のたびに `WindowEvent::Decorations` | 決めた時点で `WindowEvent::Decorations` |

`window_decorations()` はこれを追う。GNOME では xdg-decoration がないので `Client` になる。

### ウィンドウ操作

| 操作 | Wayland | X11 |
|---|---|---|
| 移動 | `xdg_toplevel.move`（最後の押下のシリアル） | ポインタの grab を外してから `_NET_WM_MOVERESIZE`（移動） |
| リサイズ | `xdg_toplevel.resize` | `_NET_WM_MOVERESIZE`（辺と角） |
| 最小化 | `set_minimized` | ルートに `WM_CHANGE_STATE`（`IconicState`） |
| 最大化 | `set_maximized` / `unset_maximized`。`configure` の状態で知らせる | `_NET_WM_STATE_MAXIMIZED_VERT` と `_HORZ`。プロパティの変化で知らせる |
| WM のメニュー | `show_window_menu` | WM が `_GTK_SHOW_WINDOW_MENU` に対応していれば使う。なければ何もしない |

### CSD のリサイズの縁

CSD では、ウィンドウの縁をつかんでリサイズできるようにする必要がある。これはバックエンドが受け持ち、core とアプリは何もしなくてよい。

- **Wayland：**
  - ウィンドウの外側に、透明なサブサーフェスを 4 本（上下左右、角は上下の帯の端）置く。
  - `xdg_surface.set_window_geometry` でウィンドウの範囲を本体だけにする。
  - 帯の上ではリサイズのカーソルを出し、押されたら `xdg_toplevel.resize` を呼ぶ。
  - 本体の座標は変わらないので、スクロールバーが端にあっても取り合わない。GTK と Chromium も、外側でリサイズを受ける。
  - 最大化、フルスクリーン、タイル表示の間は帯を外す。
  - 帯には 1×1 の透明な `wl_shm` バッファを `wp_viewport` で引き伸ばして使う。ビューポーターがなければ帯は出さない。
- **X11：**
  - 子ウィンドウは親の外に出られないので、本体の内側の縁（5 論理ピクセル）で受ける。
  - 縁の上ではポインタのイベントを core に渡さない。
  - X11 で CSD を使うのは、アプリが `Client` を選んだときだけである。

### タイトルバーの部品

`anti-webui-widgets` に置く。

```rust
column((
    title_bar("エディタ"),              // CSD のときだけ現れる
    content,
))
```

- **ドラッグ：** 押して動かすと `start_window_move`。
- **ダブルクリック：** 最大化を切り替える。
- **右クリック：** `show_window_menu`。
- **ボタン：** 最小化、最大化、閉じる。閉じるは `request_close_window` を呼ぶので、SSD の閉じるボタンと同じくハンドラを通る。
- **見た目：** テーマに従い、`styled_title_bar` で変えられる（[33](33-customization.md)）。ウィンドウがアクティブでないときは薄くする。

## 環境ごとの違い

| | Wayland | X11 | Windows（今後） | macOS（[37](37-platform-macos.md)） |
|---|---|---|---|---|
| ダイアログ（`dialog`、`modal`） | `set_parent` | `WM_TRANSIENT_FOR` | オーナー付きウィンドウ | `addChildWindow` |
| 位置の指定 | できない（コンポジタが決める） | 今はしない | できる | できる |
| モーダル | xdg-dialog があれば。core は常に親の入力を止める | `_NET_WM_STATE_MODAL` | 親を無効にする | シート |
| 飾り | 取り決める。GNOME は CSD | WM が付ける | OS が付ける | OS が付ける |
| spawn したスレッド | 同じ接続、別のキュー | 別の接続 | プラットフォームのスレッドに窓を置き、描くのは各スレッド | 窓はメインスレッド（AppKit はメインスレッドのみ）、UI と描画は各スレッド |

位置の指定と、親に貼り付く小窓（Wayland ではつかみのない xdg_popup）は、あとで足す。

## 実装の状態（2026-11-07）

- **できたもの：**
  - core の `window` と `shared`。
  - app の `Thread`（`runner.rs`）と GPU の共有（`present.rs`）。
  - Wayland と X11 の `Session` と `decor.rs`。
  - 標準の部品の `title_bar`。
  - 例 `multi_window`。
- **ShojiWM で確かめたこと（Wayland と XWayland）：**
  - 同じスレッドの窓とのシグナル共有、spawn した窓からの `shared_signal`。
  - ハンドルから閉じる、閉じる確認の `prevent`、メインと一緒に子も閉じる。
  - `dialog` の窓はダイアログとして親の上に出る。既定の窓は普通の窓として出て、最大化ボタンもある。
  - `DecorationMode::Client` で `title_bar` が出る。
- **まだのもの：**
  - 手で確かめること：入力の振り分け、CSD の縁でのリサイズ、モーダル、移動、最大化、WM のメニュー。
  - Wayland のプロトコルテスト（2 枚目の toplevel、入力の振り分け）。
  - 位置の指定、親に貼り付く小窓、同じスレッドの窓の間のアプリ内 DnD（今は他のアプリとの DnD と同じ道を通る）、`EditorRef` が結び付けたウィンドウを覚えること。

## 確かめ方

- **core の単体テスト：**
  - 2 枚のウィンドウで、文書が別になること。
  - A のハンドラで書いたシグナルで、B のエフェクトが B の文書を触ること。
  - タイマー、`NodeRef`、`ui_channel` が作られたウィンドウで走ること。
  - 閉じるとオーナーのクリーンアップが走ること。
  - 閉じる前のハンドラの `prevent`。
  - モーダルの子がある間、親の入力が止まること。
- **`shared_signal`：** 別のスレッドからの `set` で、読んだスレッドのエフェクトが走り直すこと。
- **Wayland のプロトコルテスト：** 小さなコンポジタで次を確かめる。
  - 2 枚目の toplevel の `set_parent`。
  - 入力の振り分け（`enter` が他のインスタンスの surface なら捨てる）。
  - 飾りの取り決め、`close` からハンドラへの流れ。
- **X11 のテスト（Xvfb）：**
  - 2 枚目のウィンドウの `WM_TRANSIENT_FOR`。
  - `_MOTIF_WM_HINTS`。
  - `WM_DELETE_WINDOW` から `CloseRequested`。
- **手で確かめる：** 例 `multi_window` で次を確かめる。
  - 同じスレッドの窓と spawn した窓。
  - モーダル。
  - 閉じる確認。
  - GNOME（CSD）と KDE または ShojiWM（SSD）のタイトルバー。
