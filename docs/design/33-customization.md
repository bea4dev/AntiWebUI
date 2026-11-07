# 33. フォークせずに部品を変える

crates.io に出したあと、利用者が AntiWebUI をフォークせずに、標準の部品の見た目を変えたり、自分の部品の集まり（MD3E のようなもの）を作ったりできるようにする。`anti-webui` の公開 API だけで足りることを、MD3E を標準の部品の上に載せ直して確かめる。

```
cargo run --release -p anti-webui-md3e --example custom_levels
```

## いまの問題

- **見た目の差し込み口がない。** 標準の部品で変えられるのは、テーマの色 39 個と、返ってくる `Div` の外側だけである。スイッチのつまみやスライダーの溝には届かない。
- **MD3E は標準の部品の隣に、同じ動きをもう一度書いている。** その動きがずれている。

  | 部品 | 標準 | MD3E |
  |---|---|---|
  | タブ | 一つのタブ止まりで、←→、Home/End | 矢印キーがない |
  | スライダー | PageUp/PageDown がある | ない。刻みの意味も違う（幅と数） |
  | ツールチップ | キーボードのフォーカスでも出る | 出ない |
  | トグル類 | 自分で `Signal` を切り替える | 呼び出し側が `on_click` で切り替える |

- **部品を作る道具が非公開で、二重にある。** `keep_time` は標準と MD3E に別々にある。ばねで追うもの（MD3E の `following`）とフェード（標準の `fade`）はどちらも内部にしかない。
- **MD3E が内部の抜け道を使っている。** `anti_webui::core::tree::with_doc` で文書を丸ごと借りて拡大率を読んでいる。`NodeRef::rect()` が物理ピクセルで返るからである。
- **公開範囲が広すぎる。** `anti-webui` は内部のクレートを丸ごと再公開しているので、兄弟のクレートのためだけの `pub` も semver で縛られる。

## 段階

見た目をどこまで変えるかで、使うものが変わる。下の段階ほど手間が少ない。

| 段階 | 変えるもの | 書き方 | できないこと |
|---|---|---|---|
| Lv0 | なし | `switch(on)` | — |
| Lv1 | テーマの色 | `set_theme(Theme { accent, .. })` | 一部だけ変える。寸法や形を変える |
| Lv2 | 外側の箱 | `switch(on).w(56.0).bg(..)` | 中の部品に届かない |
| Lv3 | 名前の付いた部品 | `styled_switch(on, SwitchParts::new().thumb(\|t, s\| ..))` | 部品の数と並びを変える |
| Lv4 | 見た目の全部 | `switch_with(on, \|s\| div()..)` | — |

Lv2 は CSS で要素に書くのと同じで、テーマより強く、どの状態でも勝つ（[21](21-widgets.md)）。Lv3 は CSS の `::part()` や `::-webkit-slider-thumb` に当たる。Lv4 は動きだけを借りて木を全部自分で組む形で、React Aria や Radix の見た目を持たない部品、Jetpack Compose の `foundation` と `material3` の分け方と同じである。MD3E は Lv4 で載せる。

## 部品の形

動きのある部品 `x` は、三つの入口を持つ。

```rust
x(model, ..)                     // 標準の見た目
styled_x(model, .., XParts)      // 標準の見た目の部品を変える（Lv3）
x_with(model, .., |s: XState| -> Div)   // 見た目を全部自分で組む（Lv4）
```

- **動きは、見た目が返した箱に足す。** `x_with` は先に `XState` を作って見た目に渡し、返ってきた箱にポインタ、キー、フォーカスの処理を足す。見た目の側が同じ箱に書いた `on_click` なども残る（リスナーは重ねて呼ばれる）。
- **`XState` は `Copy` で、読むと追跡する。** 中身はシグナルなので、`dyn_style` やばねの中でそのまま読める。書き換えは動きの側がする。
- **部品の中の要素に動きを付けるときは、状態のメソッドを通す。** スライダーのつまみやタブの一つ一つは、見た目の側が作って `s.thumb(i, el)`、`s.tab(i, el)` に渡す。フォーカスやクリックはそこで付く。
- **`XParts` の各部品は `Fn(Div, XState) -> Div`。** 標準のスタイルを付けた要素を受け取って返す。書いたものはテーマより強い。部品が複数あるもの（タブの一つ一つ、範囲のスライダーのつまみ）は何度も呼ばれる。タブの一つ一つだけは `Fn(Div, TabsState, usize)` で番号も受ける。
- **浮かぶもの（ツールチップ、ダイアログ）は二段にする。** 一段目は一度だけ呼ばれ、ばねなどを作って、開くたびに呼ばれる二段目を返す。閉じる動きのあいだ残すには、一段目で `s.hold_while(..)` を呼ぶ。

  ```rust
  tooltip_with(target, TooltipOptions::new(), |tip| {
      let fade = follow(move || if tip.open() { 1.0 } else { 0.0 }, fast_effects());
      tip.hold_while(move || fade.get() > 0.0);
      move || div().opacity(fade.get()).child("...")
  })
  ```

- **無効は core の `.disabled()` に任せる。** 無効の箱ではポインタもキーもフォーカスも届かないので、動きの側は何もしない。標準の見た目は `Sheet::disabled` で表す（祖先の無効も受け継ぐ）。Lv4 の見た目で無効を表すなら、同じ条件を見た目の側でも読む。

### 値の受け取り方：`Binding<T>`

標準の部品は `Signal` を受けて書き換え、MD3E は「値を返す関数と、変わったときの関数」を受けている。後者は「全部を選ぶ」チェックボックスのように、値がほかから決まるときに要る。両方を受けられるように core に `Binding<T>` を足す。

```rust
switch(on)                                            // Signal<bool> はそのまま渡せる
checkbox(Binding::new(move || all_on(), move |v| set_all(v)), "すべて")
```

- `impl From<Signal<T>> for Binding<T>`。部品は `impl Into<Binding<T>>` を受ける。今の `Signal` を渡す書き方はそのまま通る。
- `Copy`。関数の組は内部でシグナルに入れて持つ。`Signal` から作ったものは、そのシグナルを直接読み書きする。
- `get()`（追跡する）、`get_untracked()`、`set(v)`。

## core に足すもの

部品を作る人が使う道具。すべて `prelude` に入れる。

| 名前 | 中身 | 置き換えるもの |
|---|---|---|
| `Binding<T>` | 上のとおり | — |
| `Interaction` と `.interaction(i)` | ホバー、押下、フォーカス、キーボードのフォーカスの 4 つのシグナル。箱に付けると追う | 各部品の手書き |
| `follow(target, motion)` | `target` が変わるたびに、ばね（`Spring`）かトランジション（`Transition`）で追う `Animated` | MD3E の `following`、標準の `fade` |
| `clock()` | 作ってからの秒数を、生きているあいだ毎フレーム進めるシグナル | 標準と MD3E の `keep_time` |
| `NodeRef::logical_rect()` | 枠を論理ピクセルの `RectF` で返す | MD3E の `with_doc` |
| `close_window()` | 窓を閉じる | ベンチの `with_doc(\|d\| d.request(Close))` |

拡大率は、すでにある `window_scale()`（読むと追跡する）を使う。MD3E が `with_doc` で読んでいたのは、すべて拡大率だった。

あわせて、`on_resized` と `on_scrolled` も、ほかのリスナーと同じく重ねて呼ぶようにする。今は後から付けたものが前のものを消すので、動きの側と見た目の側の両方が付けると片方が消える。

## 標準の部品から出すもの

標準のテーマに合わせて自分の部品を作るときの道具を、`anti_webui::widgets` から出す。

| 名前 | 中身 |
|---|---|
| `themed(build: fn(&Theme) -> Sheet)` | テーマが変わるまで使い回す `Sheet`。`.sheet(\|\| themed(my_sheet))` と書く |
| `focus_ring(style, &Theme)` | キーボードのフォーカスの輪 |

## 今回の部品

| 部品 | 入口 | `XState` で読めるもの | `XParts` |
|---|---|---|---|
| スイッチ | `switch`、`styled_switch`、`switch_with` | `on()`、`hovered()`、`pressed()`、`focus_visible()` | `track`、`thumb` |
| チェックボックス | `checkbox`、`tri_checkbox`、`styled_checkbox`、`checkbox_with`、`tri_checkbox_with` | `state()`（`CheckState`）、`checked()`、操作の 4 つ | `row`、`indicator`、`label` |
| ラジオボタン | `radio`、`styled_radio`、`radio_with` | `selected()`、操作の 4 つ | `row`、`indicator`、`label` |
| スライダー | `slider`、`stepped_slider`、`range_slider`、`styled_slider`、`slider_with`、`range_slider_with` | `value()`、`fraction()`、`fraction_of(i)`、`dragging()`、`held()`、`hovered()`、`focus_visible()`、`thumb(i, el)` | `track`、`fill`、`thumb` |
| タブ | `tabs`、`styled_tabs`、`tabs_with` | `selected()`、`count()`、`focus_visible()`、`tab(i, el)` | `row`、`tab`、`panel` |
| ツールチップ | `tooltip`、`styled_tooltip`、`tooltip_with` | `open()`、`hold_while(f)`、`stay(el)` | `bubble` |
| ダイアログ | `dialog`、`alert_dialog`、`styled_dialog`、`modal_with` | `open()`、`dismiss()`、`scrim(el)`、`surface(el)`、`hold_while(f)` | `scrim`、`surface` |
| トグルボタン | `toggle_button`、`toggle_with` | `on()`、操作の 4 つ（スイッチと同じ `ToggleState`） | — |
| ラジオのグループ | `radio_group`、`radio_group_with` | `selected()`、`is_selected(i)`、`focus_visible()`、`item(i, el)` | — |
| メニュー | `menu`、`styled_menu`、`menu_with`（`dropdown_button`、`context_menu`） | `open()`、`lit()`、`is_lit(i)`、`keyed()`、`is_open(i)`、`item(i, el, MenuChoice)`、`submenu(i, el, setup)`、`hold_while(f)`、`close()`、`close_all()` | `surface`、`row` |
| トーストとスナックバー | `toast`、`toast_host`、`toast_host_with`、`MessageQueue` | `push`、`close`、`close_first`、`list()`、`timed(id, lasts, el)` | — |
| プルダウン | `select`、`select_with` | `value()`、`selected()`、`is_open()`、`is_lit(i)`、`option(i, el)` | — |
| リスト | `list_box`、`list_box_with`、`multi_list_box`、`multi_list_box_with` | `is_selected(i)`、`cursor()`、`focus_visible()`、`option(i, el)` | — |
| ツリー | `tree_view`、`tree_view_with` | 行ごとの `TreeRowState`：`key()`、`label()`、`depth()`、`opens()`、`is_open()`、`is_selected()`、`toggle(el)` | — |

### それぞれの動き

- **メニュー**
  - フォーカスはメニューの箱に置き、キーで「光っている行」を動かす（ARIA の `aria-activedescendant` と同じ形）。
  - 上下のキーは無効な行と区切りを飛ばし、端で反対側へ回る。Home/End もある。Enter と Space で選び、Right と Left でサブメニューに出入りし、Tab ですべて閉じる。
  - 選ぶと、その前に入れ子のメニューごと `on_dismiss` を呼ぶ。サブメニューもポインタを少し置くと開く。
  - オプション（`MenuOptions`）：置き場所、間隔、つなぐ先（`anchor`）、押した位置（`at`、コンテキストメニュー用）、つなぐ先と同じ幅、`not_modal`。
  - 開くたびに行を作り直す。行の番号は見た目の側が付け、`item` と `submenu` で作った番号だけをキーがたどる。
- **トーストとスナックバー**
  - `MessageQueue` は来た順の伝言の列で、`timed` が時間で閉じる。ポインタが載っているあいだは待ち、要素が消えたら待つのもやめる。
  - いくつ並べて出すか（トーストは重ねて全部、スナックバーは 1 つずつ）は見た目の側が決める。
- **プルダウン、リスト、ツリー**
  - 今までの標準の動きをそのまま `x_with` に移した。ツリーは見えている行だけを作る仕組みのまま、行の見た目だけを差し替える。

- **スライダー**
  - オプション（`SliderOptions`）：刻みの幅、縦向き、溝の両端の内側への寄り（`inset`）。
  - 押すと近いほうのつまみを動かし、二つのつまみは追い越さない。
  - キーは矢印、PageUp/PageDown、Home/End。
  - 見た目が `s.thumb(i, el)` を呼べば、つまみごとにフォーカスが止まる（範囲のスライダーには要る）。呼ばなければ箱全体に一つ止まる。
- **タブ**
  - 一つのタブ止まりで、←→ は端で反対側へ回る。Home/End もある。
  - キーで動かしたら、すぐ選ぶ。
  - タブの中身（パネル）は標準の `tabs` だけが持ち、`tabs_with` は見出しの列だけを扱う。
- **ツールチップ**
  - オプション（`TooltipOptions`）：出るまでの時間（既定 500ms）、消えるまでの時間（既定 0）、キーボードのフォーカスで出すか（既定で出す）、浮かぶものの種類、置き場所、間隔。
  - `s.stay(el)` を付けた要素にポインタがあるあいだは消えない（中を押せるリッチなツールチップ）。
  - 押すと消える。
- **ダイアログ**（`modal_with`）
  - ポータルに置き、Tab を中に閉じ込める。開いたら中にフォーカスを移し、閉じたら戻す。
  - Escape と `s.scrim(el)` を付けた要素のクリックで `on_dismiss` を呼ぶ。
  - `s.surface(el)` を付けた要素で押したものは、外へ伝えない。

## MD3E の載せ直し

上の部品は、MD3E でも標準の動きを使う。見た目（シェーダー、ばね、リップル）はそのまま残す。

| MD3E の部品 | 使う動き |
|---|---|
| `switch`、`styled_switch` | `switch_with` |
| `checkbox`、`tri_state_checkbox`、`styled_checkbox` | `checkbox_with`、`tri_checkbox_with` |
| `radio_button`、`styled_radio_button` | `radio_with` |
| `slider`、`styled_slider`、`range_slider` | `slider_with`、`range_slider_with` |
| `tabs`、`styled_tabs` | `tabs_with` |
| `tooltip`、`rich_tooltip` | `tooltip_with` |
| `dialog`、`alert_dialog`、`bottom_sheet`、`side_sheet`、`navigation_drawer` | `modal_with` |
| `menu`、`styled_menu`（`dropdown_field` も） | `menu_with` |
| `toggle_button`、`styled_toggle_button`、`icon_toggle_button`、`filter_chip`、`styled_filter_chip`、`selectable_input_chip`、`connected_button_group` | `toggle_with` |
| `snackbars` | `MessageQueue` |

ナビゲーションバーとレール、日付と時刻のピッカー、カルーセルは、標準に同じ部品がないので、動きは MD3E の中のままである。ナビゲーションの項目は、Material と ARIA のとおり Tab で一つずつ止まるボタンなので、タブとは分けた。

### 使い方が変わるところ

- **値は `Binding` で受ける。** 切り替えは部品がする。

  ```rust
  // 前
  md3e::switch(move || on.get()).on_click(move |_| on.update(|v| *v = !*v))
  md3e::slider(move || v.get(), move |x| v.set(x))
  // 後
  md3e::switch(on)
  md3e::slider(v)
  ```

- ラジオボタンは `radio_button(selected, value)` になる。
- 範囲のスライダーは `range_slider(start, end, style)` になる。
- タブは `tabs(labels, selected)` になる。
- `CheckState` は標準のものを使う。部分的に入っている状態は `Mixed` と呼ぶ（ARIA の `mixed` と同じ）。

### 動きが変わるところ

- タブは一つのタブ止まりになり、矢印キーで動く。
- スライダーに PageUp/PageDown が付く。
- ツールチップがキーボードのフォーカスでも出る。
- メニューは開くとフォーカスを取り、キーが無効な項目を飛ばし、選ぶと閉じる（`on_dismiss` が呼ばれる）。
- トグルボタンとチップは自分で切り替える（値は `Binding`）。`connected_button_group` は今までどおり位置で受ける。
- スナックバーはポインタが載っているあいだ待つ。

時間と見た目は MD3E のまま残す。出るまで 400ms、消えるまで 200ms、Compose の寸法とばねである。

## 公開範囲

- **内部用は隠す。** 兄弟のクレートとテストのためだけの `pub` には `#[doc(hidden)]` を付ける。
  - 隠したものは semver の約束に含めない。core の crate のドキュメントにそう書いた。
  - `pub(crate)` にしないのは、テスト（core だけで 29 ファイル）と兄弟のクレートが使うからである。
  - 足りないものが見つかったら、隠すのではなく利用者向けの API を足す（`logical_rect`、`close_window`）。

  | クレート | 隠したもの |
  |---|---|
  | core | モジュール `tree`、`scene`、`paint`、`damage`、`layout`、`overlay`、`ui`（`Ui` は根元からの再公開で見える）。`Ui` の動画、画像の起こし方、数え上げのメソッド。`ImeState`、`PopupId`、`DragOffer`、`ImeUpdate`、`AXIS_MOTION_GAIN`。シェーダーのマクロの中身（`ParamType`、`ParamField`、`ParamWriter`、`ParamValue`）と描画側の `ShaderSource` など。`text::edit`、`text::selection`、`TextSystem`、`with_text`、`ShapeStyle`、`TextLayout`、`parley` の再公開。動画のバックエンドの取り決め（`VideoBackend` など）。`ImageStore`。`create_child_owner`、`effect_with` |
  | style | `ComputedStyle`、`AnimatedValue`、`AnimatedField`、`ANIMATED_FIELDS` |
  | wgpu | `dmabuf`、`linux_sync`、`limit_vulkan_drivers`、`WindowRenderer`、`FrameResult`、`Renderer` の窓とテスト用のメソッド |
  | 外のクレートの再公開 | `anti_webui::wgpu::wgpu`（wgpu そのもの）、`svg::kurbo`。版が上がると利用者が壊れるので、約束に含めない |

  `Ui`（窓なしで動かす道具）、`WindowEvent`、`PlatformRequest`、`Scene` は、利用者が自分の部品をテストするのに要るので残した。
- **MD3E を外の利用者の見本にする。** MD3E が `anti_webui::prelude`、`anti_webui::widgets` と、テスト用の `anti_webui::wgpu::check_shader` 以外を使っていないことを、テストで確かめる。

## core の決まりの変更

- **`.disabled(f)` は重ねられる。** 何度か書くと、どれかが真のあいだ無効になる。今までは後のものが前のものを消していた。動きの層（メニューの無効な項目）と見た目の側の両方が書くためである。
- **標準のメニューは閉じるとすぐ消える。** 今までどおりで、出るときだけフェードする。`menu_with` では、閉じる動きを残したい見た目が `hold_while` を使う（MD3E のメニュー）。
- **`tooltip_with` の中身は `Div`。** 役割（`Tooltip`）を付けるためである。

## 読み上げ

役割と状態は動きの層が付ける（[34](34-accessibility.md)）。見た目を丸ごと差し替えても、支援技術に言うことは変わらない。

## やらないこと（次）

- テーブルの動きの分離。セルの見た目は `custom_column` で変えられる。
- 見た目からの無効のシグナル。今は Lv4 の見た目が自分で同じ条件を読む。
- プルダウンやリストで、文字を打って項目へ飛ぶこと。

## 確かめたこと

- **テスト：** ワークスペースの 1,428 個が通る（二回目の分離と読み上げの口を足したあと）。
  - 標準の部品に `tests/looks.rs`（10 個）を足した。`Binding` の関数の組、自前の見た目のスイッチ、大きいつまみ、範囲のスライダー、縦向きと `inset`、`tabs_with`、ツールチップの待ち、`modal_with` の Escape とスクリム、閉じる動きの保持、リスナーが重なること。
  - MD3E には、動きが変わったところのテスト（タブの矢印キー、スライダーの PageUp/PageDown、キーボードのフォーカスで出るツールチップ）と、公開 API だけを使っていることのテストを足した。
- **見た目：** 一回目に載せ直した部品は、MD3E が前と画素単位で同じだった。二回目（メニュー、トグル、チップ、スナックバー）は見た目のコードに手を入れていないので、MD3E の寸法と色のテストで確かめた。比べたのは、スイッチ 6 種、チェックボックス 6 種、ラジオ 3 種、スライダー 7 種（段階、中央から、範囲、無効、縦向き）、タブ 4 種の静止画と、開いた `dialog`、`alert_dialog`、`bottom_sheet`、`side_sheet`、`navigation_drawer`。
- **`custom_levels`：** Lv0 から Lv4 までと本物の MD3E を並べた。動きを触って確かめるのは手で行う。
