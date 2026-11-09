# 20. MD3E の残りの部品とショーケース

[15](15-md3e.md) と [19](19-buttons.md) に続けて、Material 3 Expressive の部品を一通り揃える。値は同じく Jetpack Compose の Material 3（`androidx-main`、2026-11-04 時点）から取る。Compose が動かしていないところには、同じモーショントークンのばねを足した。そうした箇所は各節の「Compose との違い」に書く。

```
cargo run --release -p anti-webui-md3e --example showcase [actions|selection|communication|containment|navigation|inputs]
```

## 部品の一覧

| 分類 | 部品 |
|---|---|
| 選択 | `switch`、`checkbox`、`tri_state_checkbox`、`radio_button`、`slider`、`range_slider`、`assist_chip`、`suggestion_chip`、`filter_chip`、`input_chip` |
| 通知 | `linear_progress`、`circular_progress`、`circular_wavy_progress`、`indeterminate_*` 4 種、`badge`、`text_badge`、`badged`、`tooltip`、`rich_tooltip`、`snackbars` |
| 容器 | `filled_card`、`elevated_card`、`outlined_card`、`clickable_card`、`divider`、`vertical_divider`、`list_item`、`segmented_list`、`dialog`、`alert_dialog`、`bottom_sheet`、`side_sheet`、`navigation_drawer`、`carousel`、`card_media`、`carousel_image` |
| 画像 | `image`、`avatar`、`initials_avatar`（[22](22-images.md)） |
| 並べ替え | `sortable_list`、`sortable_row`（[23](23-drag-and-drop.md)） |
| 移動 | `navigation_bar`、`short_navigation_bar`、`navigation_rail`、`wide_navigation_rail`、`drawer_item`、`tabs`、`styled_tabs`、`top_app_bar`、`top_app_bar_over`、`bottom_app_bar`、`flexible_bottom_app_bar`、`floating_toolbar`、`floating_toolbar_with_fab` |
| 形 | `shape`、`styled_shape`、`MaterialShape`（[15](15-md3e.md) の「形」） |
| 部品を作るための部品 | `Ink` |
| 入力 | `text_field`、`outlined_text_field`、`dropdown_field`、`search_bar`、`menu`、`fab_menu`、`split_button`、`date_picker`、`date_picker_dialog`、`time_picker`、`time_picker_dialog` |

スイッチ、チェックボックス、ラジオボタン、スライダー、タブ、ツールチップ、ダイアログとシートは、標準の部品の動き（`switch_with` など）の上に MD3E の見た目を載せている（[33](33-customization.md)）。値は `Binding`（`Signal` をそのまま渡せる）で受け、変更も部品がする。そのほかの部品は、状態を値を読むクロージャで受け、変更はアプリが行う（[19](19-buttons.md) のトグルと同じ）。

```rust
switch(on)
slider(volume)
filter_chip("Mine", move || mine.get()).on_click(move |_| mine.update(|v| *v = !*v))
text_field(name, TextFieldStyle::new().label("Name").supporting(|| "Required".into()))
alert_dialog(move || open.get(), move || open.set(false), AlertDialog::new().title("Delete?"))
```

## コアに足したもの

| 追加 | 理由 |
|---|---|
| `portal()`、`Popup::modal()` | ダイアログ、シート、スナックバー、検索を、宣言した場所によらずウィンドウの上に出す |
| `set_timeout(delay, f)` → `Timeout::cancel()` | ツールチップの待ち、スナックバーの時間。待っている間はフレームを要求しない |
| `.on_scrolled(f)`、`NodeRef::scroll_offset`、`NodeRef::scroll_to` | アプリバーの折りたたみ、スクロールするタブ |
| `.on_resized(f)` | 大きさで決まるもの（カルーセル、シート、タブの目印）を、描く前に更新する |
| `Placement::AboveCenter`、`BelowCenter` | ツールチップ |
| `.node_ref` を複数付けられること | 部品が内部で使う `NodeRef` を利用者の指定が奪わないように |
| `.pointer_events_auto()` | 素通しする親の中で、一部だけ入力を受ける |

### `portal`

[09](09-shadows-stacking-popups.md) のポップアップと同じく、**論理的には宣言した場所の子**で、描画とレイアウトでは別の根になる。違いはサーフェスを持たないこと。

| | `popup` | `portal` |
|---|---|---|
| 描く先 | 別のサーフェス | ウィンドウのサーフェス（内容の上） |
| 大きさ | 内容に合わせる | ウィンドウと同じ |
| ウィンドウの外 | 出られる | 出られない |
| コスト | サーフェスとバッファ | 増えない |
| 子のないところの入力 | 届かない | 下へ素通りする |

- `layout_document` がウィンドウの大きさで計算し、`position_popup` が原点に置く。`paint_root` がウィンドウの内容の後に描く。
- ヒットテストは、本物のポップアップ、ポータル（新しいものから）、ウィンドウの順。ポータル自身に当たったときは次へ進むので、ポータルは「中身だけ」が入力を受ける。
- `popup_of` はポータルを飛ばす。メニューを閉じる判定やサーフェスの親は、ウィンドウの上にあるものとして扱う。
- スクロールの連鎖はポータルで止まる。
- `.modal()` は、存在する間 Tab をその中に閉じ込め、消えるときにフォーカスを前の場所へ戻す。

### `set_timeout`

スレッドローカルの一覧に持つ。`Ui::next_deadline` が一番早い時刻も返し、`Ui::tick` が期限の来たものを実行する。プラットフォーム側は変えていない（キャレットの点滅と同じ経路）。作ったオーナーが破棄されると取り消される。

### 通知の順番

`on_scrolled` と `on_resized` はレイアウトの中では呼ばず、`mark_scrolled` と `position_node` で一覧に積んで `Ui::update` が呼ぶ。`on_resized` はレイアウトのループの中で呼ぶので、大きさに応じて変えたスタイルは同じフレームのうちに反映される。

### 折り返さない文字の幅

`whitespace_nowrap` の文字は、親の空きが足りなくても本来の幅のまま測る（CSS と同じ）。以前は折り返さないのに幅だけ切り詰めていて、閉じた箱の中で本来の幅を測る部品（拡張 FAB、FAB メニュー）が小さな値を読んでいた。

### `Animated` の精度

ばねの積分を、目標からの差で行うように変えた。`f32` のまま絶対値で積分すると、値が大きいとき（日付ピッカーの月番号は約 24000）に 1 ステップの移動が丸めで消え、静止しないまま毎フレームを要求し続けた。

## 共通の部分

`surface.rs` から状態レイヤーとリップルを `Ink` として切り出した。**動きを受ける箱と描く箱を分けられる**ので、スイッチのように「ルートでホバーを受け、つまみの周りの 40 の円に描く」部品が書ける。

`Ink` は公開している。アプリが自分で作った `div()`（デスクトップのバーやドックのタイルなど）にも、部品と同じホバー、フォーカス、押下の表示を付けられる。

```rust
use anti_webui_md3e::components::Ink;

let ink = Ink::new(|c| c.on_secondary_container, move || off.get());
let tile = div().w(120.0).h(56.0).rounded(16.0).focusable()
    .dyn_style(|s| s.bg(color_scheme().secondary_container));
let tile = ink.paint(ink.listen(tile));   // 受ける箱と描く箱が同じ
```

| API | 内容 |
|---|---|
| `Ink::new(color, disabled)` | `color` は `Fn(&ColorScheme) -> Color`（ふつうは内容の色）、`disabled` は `Fn() -> bool`。無効の間は何も描かない |
| `listen(div)` | ホバー、`:focus-visible`、押下、Enter と Space を受ける。リップルは押した位置から広がる |
| `listen_centered(div)` | 同じだが、リップルは描く箱の中心から広がる。受ける箱と描く箱が違うとき用 |
| `paint(div)` | その箱の `overlay` に、箱の角丸の形で描く。`bg` の上、子の下 |
| `hovered`、`focused`、`pressed` | 状態のシグナル。`focused` は外から立ててもよい（キーで項目を移るリスト） |

濃さと時間は [15](15-md3e.md) の「状態レイヤーとリップル」と同じ。何も起きていない間は `overlay` が `None` を返し、フレームも要求しない。`paint` は `overlay` を使うので、その箱に別の `overlay` は付けられない。以前の `disabled` は内部の `Rc<dyn Fn() -> bool>` だったが、公開にあたって `impl Fn() -> bool` にした（部品の中では `new_when` で `Rc` のまま渡す）。

core の `follow(target, spring)` は、目標が変わったときだけばねを始める値（以前はこのクレートの `following`）。止まっているときにフレームを要求しないという、[15](15-md3e.md) の約束をここで守る。

## 選択

### スイッチ

| 項目 | 値 |
|---|---|
| トラック | 52 × 32、枠線 2 |
| つまみ | 通常 16、アイコンありと選択時 24、押下 28 |
| 左端の位置 | 通常 8、アイコンあり 4、選択時 24、押下 2（選択時 22） |
| 当たり判定 | 52 × 48 |
| 状態レイヤー | 40 の円 |

| 部分 | オフ | オン | 無効オフ | 無効オン |
|---|---|---|---|---|
| トラック | surfaceContainerHighest | primary | 同 12% | onSurface 12% |
| 枠線 | outline | primary | onSurface 12% | onSurface 12% |
| つまみ | outline | onPrimary | onSurface 38% | surface |
| アイコン | surfaceContainerHighest | onPrimaryContainer | 同 38% | onSurface 38% |

無効の色は surface に重ねて不透明にする（Compose と同じ）。

**Compose との違い。** Compose は押した瞬間につまみを飛ばし、色も瞬時に変える。ここでは押下を速い効果のばね（3800、1.0）で、色を 100 ms で動かす。離したときの速い移動のばねは同じ。ドラッグでの切り替えは Compose にもない。

### チェックボックス

| 項目 | 値 |
|---|---|
| 箱 | 20、角丸 2、線 2 |
| 当たり判定 | 48 |
| 状態レイヤー | 40 の円 |
| チェックの点 | (0.2, 0.5) → (0.4, 0.7) → (0.8, 0.3)（箱の幅に対する割合） |
| 横棒の点 | (0.2, 0.5) → (0.5, 0.5) → (0.8, 0.5) |

印は `check.wgsl` で描く。`fraction` の長さまで左から描き、`gravity` で真ん中の点を中心へ寄せると横棒になる。

- オフ → オン: 長さが標準の移動のばねで伸びる。
- オン ↔ 不確定: 長さはそのままで `gravity` が同じばねで動く。
- オン → オフ: 長さはそのままで色が速い効果のばねで消える。

**Compose との違い。** Compose の幾何は `isCheckboxStylingFixEnabled = false` の 20 の箱を使っているので、それに合わせた（フラグを立てた 18 の箱ではない）。Compose は 100 ms 後に長さを 0 へ飛ばすが、ここでは次に描くときに 0 から始める（見た目は同じ）。`error_when` はトークンにあって Compose の引数にないもの。

### ラジオボタン

輪 20、線 2、点 10。点の大きさは速い移動のばねで、選択時の色は標準の効果のばね。Compose は半径を描き直すが、ここでは `scale` で動かす（合成だけで、形が崩れない）。

### スライダー

| 項目 | 値 |
|---|---|
| トラック | 16、外側の角丸 8、内側の角丸 2 |
| つまみ | 4 × 44、押下とキーボードのフォーカスで 2 |
| つまみとの間 | 6 |
| 止まりの点 | 4 |
| 当たり判定 | 高さ 48 |

全部を `slider.wgsl` の `fill` で描くので、動いている間もレイアウトを通らず、端数の位置で描ける。トラックの切り方は `Slider.kt` の `drawTrack` と同じ式。

`SliderStyle::wavy()` は、始まりからのスライダーを波打つ進捗表示の見た目にする。覆われた側は進捗表示と同じ波線（振幅 3、波長 40、線 4）で、残りのトラックも線と同じ太さ 4、つまみは高さ 24。Compose にはない。Android 16 の再生位置のバーに当たる。`wave(振幅, 波長, 線)` は波線だけを変え、トラックとつまみはそのまま。

- ポインタが乗っているか押されている間だけ、波が 1 秒に 1 波長流れる。それ以外は止まっていて、フレームを要求しない。
- 覆われた長さが 1 波長より短いと平らに近づく。端に半端な波を出さないため。
- 範囲のスライダーと中央からのスライダーは、まっすぐのまま。

**Compose との違い。** `Slider.kt` にはアニメーションが一切ない。

| 項目 | Compose | ここ |
|---|---|---|
| つまみの幅 | 瞬時 | 速い移動のばね |
| クリックとキーでの値 | 瞬時 | 速い移動のばね（ドラッグ中はポインタに付く。刻みがあるときは刻みへ弾む） |
| 押したときの値 | 離すか動かすまで変わらない | すぐ変わる |
| トラックの狭まり | しきい値で消える | 角丸ごと 0 まで狭まり、止まりの点も一緒に消える |
| 値のラベル | 呼び出し側が組む | `.label(f)`。つまみの上に出て、拡大（速い移動）とフェード（速い効果）で現れる |

Compose には XS〜XL のサイズトークンがない（`SliderTokens.kt` だけ）。太いトラックはサンプルと同じく `.track(36.0).corner(12.0)` で指定する。`vertical()` は上へ増える。

### チップ

| 種類 | 枠線 | 背景（選択時） | ラベル | 先頭アイコン |
|---|---|---|---|---|
| assist | outlineVariant | なし | onSurface | primary |
| suggestion | outlineVariant | なし | onSurfaceVariant | primary |
| filter | outlineVariant | secondaryContainer | onSurfaceVariant → onSecondaryContainer | onSurfaceVariant → onSecondaryContainer |
| input | outlineVariant | secondaryContainer | onSurfaceVariant → onSecondaryContainer | onSurfaceVariant → primary |

高さ 32、アイコン 18。`elevated()` は枠線なし、surfaceContainerLow、影は Level 1（ホバーで Level 2）。

filter と input は Compose の `shapes` 版の形にする。

| 状態 | 角丸 |
|---|---|
| 通常 | 12 |
| 押下 | 8 |
| 選択 | 丸 |

形は速い移動のばね。filter はアイコンがなければ、チェックが幅 0 から伸びて現れる（速い移動）。Compose ではチェックは呼び出し側が渡すものだが、ここでは部品がやる。

## 通知

### 進捗

| 部品 | 大きさ | 線 | 備考 |
|---|---|---|---|
| `linear_progress` | 240 × 4 | 4 | 値は Compose の推奨のばね（50、1.0）で動く |
| `indeterminate_linear_progress` | 240 × 4 | 4 | 1750 ms の周期で 2 本 |
| `indeterminate_wavy_progress` | 240 × 10 | 4 | 同じ周期、波長 20 |
| `circular_progress` | 40 | 4 | 値は同じばね |
| `circular_wavy_progress` | 48 | 4 | 値は 500 ms の直線。波は 10〜95% の間だけ。波がないときはフレームを要求しない |
| `indeterminate_circular_progress` | 40 | 4 | 6000 ms の周期 |
| `indeterminate_circular_wavy_progress` | 48 | 4 | 同じ周期 |

不確定のキーフレームは Compose の定数をそのまま使う。形が動く部品だけ毎フレームを要求し、ノードが消えれば止まる。

**Compose との違い。** 波打つ円は `RoundedPolygon` の変形ではなく、半径を余弦で揺らして距離で塗る（波の数は同じ式、振幅 1.6）。不確定から確定への受け渡しはない（別の部品にした）。波の出入りは 500 ms ではなく標準の効果のばね。

### バッジ

`badge()` は 6 の点、`text_badge(f)` は高さ 16 で文字が空の間は消える。`badged(anchor, badge, large)` が角に載せる。Compose にアニメーションはないが、ここでは文字の有無で速い移動のばねの拡大で出入りする。

### ツールチップ

| | `tooltip` | `rich_tooltip` |
|---|---|---|
| 色 | inverseSurface / inverseOnSurface | surfaceContainer / onSurfaceVariant |
| 角丸 | 4 | 12 |
| 最大幅 | 200 | 320 |
| 影 | なし | Level 2 |
| 種類 | `PopupKind::Tooltip` | `PopupKind::Bubble`（中の操作を受ける） |
| 位置 | 上の中央、間 4 | 上の左端、間 4 |

拡大は 0.8 → 1 を速い移動のばね、透明度は速い効果のばね。

**Compose との違い。** Compose はホバーですぐ出す（`Hover` に待ちがない）。デスクトップのポインタではうるさいので、400 ms 待つ。リッチのほうは、ツールチップへポインタを移せるよう 200 ms 待って閉じる。三角の吹き出しはない。

### スナックバー

```rust
let bars = snackbars();
root.child(bars.view());
bars.show(Snackbar::new("保存しました").action("元に戻す", || undo()).closable());
```

| 項目 | 値 |
|---|---|
| 色 | inverseSurface / inverseOnSurface、操作は inversePrimary |
| 角丸 / 影 | 4 / Level 3 |
| 最小の高さ / 最大幅 | 48 / 600 |
| 表示時間 | 4 秒、`long()` で 10 秒、操作があれば閉じるまで |

一度に 1 つで、順番に出る。

**Compose との違い。** Compose はばねで拡大とフェードをするが、`for_each` の `exit` は `Transition` で動くので、220 ms の少し行き過ぎる曲線にした。

## 容器

### カード、区切り線

角丸 12、影は Level。色は表のとおり。

| 種類 | 背景 | 枠線 | 影（通常 / ホバー） |
|---|---|---|---|
| filled | surfaceContainerHighest | なし | 0 / Level 1 |
| elevated | surfaceContainerLow | なし | Level 1 / Level 2 |
| outlined | surface | outlineVariant | 0 / 0 |

`clickable_card(kind)` はボタンと同じ状態レイヤーとリップルを持つ。

### リスト

| 行数 | 最小の高さ |
|---|---|
| 1 | 56 |
| 2 | 72 |
| 3 | 88 |

左右 16、上下 10、間 12。形は Expressive の版に合わせた。

| 状態 | 角丸 |
|---|---|
| 通常 | 4 |
| ホバー | 12 |
| フォーカス、押下、選択 | 16 |

形は速い移動のばね、色は標準の効果のばね。`segmented_list` は間 2 で、外側を 16 にする。

**Compose との違い。** 背景は surface ではなく透明。シートやカードの中に置いても色が浮かないようにした。

### ダイアログ

幅 280〜560、角丸 28、surfaceContainerHigh、内側 24。`alert_dialog` のアイコンは secondary で、あるとタイトルが中央に寄る。

スクリムは scrim の 32%。外側のクリックと Escape で `on_dismiss` を呼ぶ。開くとダイアログにフォーカスが移り、Tab は中に閉じ込められ、閉じると元へ戻る。

**動きは Compose にない**（共通コードにはなく、プラットフォームのウィンドウが担当する）。拡大 0.8 → 1 を標準の移動のばね、透明度を速い効果のばね、スクリムを標準の効果のばねにした。

### ボトムシート

| 項目 | 値 |
|---|---|
| 最大幅 / 最大の高さ | 640 / 90% |
| 角丸 | 上だけ 28 |
| 色 | surfaceContainerLow |
| ハンドル | 32 × 4、上下 22 |
| 開く / 閉じる | 標準の移動 / 速い効果 |
| ドラッグで閉じる | 56 より下、または毎秒 125 より速く |

標準の移動のばねは行き過ぎるので、シートの下に 48 だけ余分に伸ばしてある。弾んでも下に隙間が出ない。ドラッグはハンドルだけ。途中で止まる段階（部分展開）はない。

### ドロワー、サイドシート

`navigation_drawer` は幅 240〜360、右側の角丸 16、surfaceContainerLow。開くのは標準の移動、閉じるのは速い効果。半分以上引くか毎秒 400 より速ければ閉じる。

`side_sheet` は Compose にない。ドロワーを左右反転し、幅の上限を 400 にした。

### カルーセル

`multi_browse_keylines` は Compose の式で、与えられた幅と希望の大きさから、大・中・小の場所を決める。項目は常に大の幅で作り、場所の幅で切り抜いて見せる。Compose の `maskClip` と同じく、切り抜きの中央に項目の中央を合わせる（パララックス）。

**Compose との違い。** 端での切り替えは Compose の段階的な計算ではなく、並びを逆順にしたものへ 1 回で補間する。角丸は 28、間は 8（Compose は呼び出し側が決め、既定の間は 0）。ドラッグとスクロールのあと、次の項目へばね（400、1.0）で止まる。

## 移動

### ナビゲーションバー、レール

| 部品 | 高さまたは幅 | 目印 | ばね |
|---|---|---|---|
| `navigation_bar` | 高さ 80 | 56 × 32、中央から伸びる | 速い移動 |
| `short_navigation_bar` | 高さ 64 | 56 × 32 | 標準の移動 |
| `short_navigation_bar`（横並び） | 高さ 64 | 高さ 40、ラベルを含む | 標準の移動 |
| `navigation_rail` | 幅 80 | 56 × 32 | 速い移動 |
| `wide_navigation_rail` | 幅 96 / 220 | 開くと高さ 56、ラベルを含み左から伸びる | 標準の移動 |

ワイドレールは `AnimatedNavigationItem` の式どおりに変形する。ラベルは位置の進み具合 `q` に対して `4(q − 0.5)²` の透明度で、0.5 で一度消え、そこで文字の大きさと場所が変わる。

**Compose との違い。** 選択したアイコンは `FILL` が 1 になる（Compose はアイコンの差し替えを呼び出し側に任せる）。ワイドレールの開いた幅は 220 で固定で、内容に合わせて広がらない。モーダル版はない。

### タブ

高さ 48（アイコンと文字で 72）、目印は primary で高さ 3。標準は内容の幅で上だけ丸く、`secondary` はタブの幅で四角い。位置と幅は標準の移動のばね。`scrollable()` は最小幅 90、左右 52 で、選択したタブが中央に来るようにスクロールする。

**Compose との違い。** 選択していない色は onSurfaceVariant（Compose の既定は選択時と同じ色で、トークンが使われていない）。選択したアイコンは `FILL` が 1 になる。

### アプリバー

| 種類 | 開いた高さ | 大きいタイトル |
|---|---|---|
| Small、Center | 64 | なし |
| Medium | 112 | Headline Small |
| Large | 152 | Headline Medium |
| MediumFlexible | 112（副題で 136） | Headline Medium |
| LargeFlexible | 120（副題で 152） | Display Small |

```rust
top_app_bar_over(
    TopAppBar::new(AppBarKind::LargeFlexible, "題名").navigation(back).action(more),
    AppBarScroll::ExitUntilCollapsed,
    content,
)
```

`top_app_bar_over` はバーを内容の上に重ね、内容に開いた高さの余白を付ける。

| 動き | 振る舞い |
|---|---|
| `Pinned` | 動かず、スクロールすると色が surfaceContainer へ |
| `EnterAlways` | 内容と一緒に上へ行き、戻すとすぐ出る。止まると近いほうへ揃う |
| `ExitUntilCollapsed` | 64 まで畳まれ、内容が先頭に戻ると開く |

タイトルの透明度と背景色は、Compose と同じ曲線で折りたたみの割合に追従する。

**Compose との違い。** Compose はスクロールを先に消費して内容を止めるが、ここでは重ねて余白を変えない。見た目の動きは同じになる。`ExitUntilCollapsed` は途中で止まれる（Compose は揃える）。

### ツールバー

`floating_toolbar` は高さ 64、丸、内側 8。`expanded_when` で 0 まで閉じる（速い移動）。`floating_toolbar_with_fab` は閉じると FAB が 56 → 80 に育つ。`vertical()` と `vibrant()` がある。スクロールに連動する動きは入れていない。

## 入力

### テキストフィールド

| 項目 | filled | outlined |
|---|---|---|
| 最小の大きさ | 280 × 56 | 280 × 56 |
| 背景 | surfaceContainerHighest | なし |
| 角丸 | 上だけ 4 | 4 |
| 線 | 下に 1、フォーカスで 2 | 周りに 1、フォーカスで 2 |

| 動き | ばね |
|---|---|
| ラベルの大きさ（16 → 12）と位置 | 速い移動 |
| 線の太さ | 速い移動 |
| ラベルと線の色 | 速い効果 |
| 例文と接頭辞の透明度 | 速い効果 |

outlined の枠線は `outline_field.wgsl` で描き、ラベルの幅 × 進み具合に合わせて切れ目を開ける。ラベルの文字の大きさはアニメーションの間、毎フレーム組み直す（Compose と同じ）。

`dropdown_field` は読み取り専用のフィールドとメニューの組み合わせ。

**Compose との違い。** 文字が入る前の高さの補間（`EmphasizedAccelerate` を掛ける式）は入れていない。ラベルを枠の外に置く `Above` はない。

### メニュー

Expressive のグループの形にする。普通のメニューは 1 グループのメニューとして書く。

| 項目 | 値 |
|---|---|
| グループの間 | 2 |
| グループの角丸 | 単独 16、先頭 16 / 8、途中 8、末尾 8 / 16 |
| 項目 | 最小の高さ 44、左右 4 離して中に 12 |
| 項目の角丸 | 単独と途中 4、先頭 12 / 4、末尾 4 / 12、選択 12 |
| 色 | surfaceContainerLow、選択 tertiaryContainer（`vibrant` は tertiaryContainer と tertiary） |
| 開閉 | 拡大 0.8 → 1 を速い移動、透明度を速い効果 |

上下キーで項目を移動する。

**Compose との違い。** ホバーのあとにグループの角が小さくなる動き（`inactiveShape`）はない。拡大の原点は置く方向で決める。外をクリックしてコンポジタやコアが閉じたときは、作り直せるようすぐ消す（`exit` は見えない）。

### FAB メニュー

```rust
fab_menu(icons::ADD, move || open.get(), move || open.update(|o| *o = !*o), items)
```

項目は場所を取らず、FAB の上に重なる。遅い効果のばね（800、1.0）で 0 から項目数まで増える値が、近い項目から順に見せていく。各項目は幅を速い移動、透明度を速い効果で動かす。FAB は角丸 16 → 28、primaryContainer → primary に変わる。アイコンは 0.5 で ✕ に変わる。

**Compose との違い。** 変わるときにアイコンを 90 度回す（Compose にはない）。Compose の Medium と Large のトグルはない。

### スプリットボタン

| サイズ | 高さ | 内側の角丸 | 押下 | 右のアイコン |
|---|---|---|---|---|
| ExtraSmall | 32 | 4 | 8 | 22 |
| Small | 40 | 4 | 12 | 22 |
| Medium | 56 | 4 | 12 | 26 |
| Large | 96 | 8 | 20 | 38 |
| ExtraLarge | 136 | 12 | 20 | 50 |

間は 2。右は開くと丸くなり、矢印が 180 度回る（Compose のサンプルと同じ）。Compose の視覚的な中央合わせ（`0.11 × 角丸の差`）は入れていない。

### 検索

| 項目 | 値 |
|---|---|
| バー | 高さ 56、丸、surfaceContainerHigh |
| 開いた `docked` | 同じ幅、高さは 240〜ウィンドウの 2/3 |
| 開いた `full_screen` | ウィンドウ全体、角丸 0、surfaceContainerLow |
| 開く / 閉じる | 遅い移動 / 標準の移動 |

開いている間はポータルに同じ入力欄を作り、バーの位置から広げる。値のシグナルは共通なので、文字はそのまま続く。外のクリックと Escape で閉じる。

**Compose との違い。** 内容のフェードは 100 ms の曲線ではなく速い効果のばね。`WithGap` の版はない。

### 日付ピッカー

幅 360、日のセル 48、選択の円 40、年のセル 72 × 36。`DatePickerStyle::no_header()` は上の題と選んだ日付（と下の線）を出さない。Compose の `title = null`、`headline = null` に当たり、いつも開いているカレンダーに使う。選択の色は標準の効果のばね。月の移動はばね（1500、1.0）で、隣の月と並んで動く。年の一覧は高さを伸ばして重ねる。

`DatePickerStyle::cascade(間隔)` は、最初に出たときに曜日と日を左上から斜めの順に `間隔` ずつ遅らせ、拡大しながらフェードインさせる（曜日が列の順、日が行 + 1 + 列の順）。Compose にはない。囲むパネルと一緒に現れるカレンダーのため。遅れはそのセルが初めてレイアウトされたときから数える。作ったときから数えると、新しいウィンドウではコンポジタの configure を待つ間に終わってしまう。移った先の月は普通に出る。

**Compose との違い。** 入力モードと範囲選択はない。月のドラッグはない。月の名前は英語で、週は日曜始まり（ロケールの仕組みがまだない）。`Date::today()` は UTC。

### 時刻ピッカー

ダイヤル 256、外の数字の半径 101、内 69、取っ手 48、線 2、中心の点 8。文字盤は `time_dial.wgsl` で描く。

針は標準の移動のばねで短い向きに回る。ドラッグ中はポインタに付き、離すと刻みへ揃う。時を選ぶと 100 ms 後に分へ移る。

**Compose との違い。** 取っ手に重なった数字の色は、ブレンドではなく距離で onPrimary へ寄せる。入力モードと横長のレイアウトはない。

## 配色と文字

- `ColorScheme` に `error_container`、`on_error_container`、`surface_dim`、`surface_bright`、`surface_variant`、`inverse_primary`、`scrim` を足した。フィールドを全部書いて作るコードは直す必要がある。
- `TextStyle` を型スケールの 15 種類すべてにした。値の確認で、`TITLE_MEDIUM` の字間を 0.15 から 0.2 に直した（現在のトークンの値）。Medium サイズのボタンのラベルも変わる。

## ショーケース

`anti-webui-md3e/examples/showcase`。ワイドレールでページを切り替え、大きいアプリバーの下で各ページをスクロールする。右上で配色を切り替える。ページの切り替えは `for_each` の `enter` / `exit`。

| ページ | 内容 |
|---|---|
| `actions` | ボタン、アイコンボタン、FAB、グループ、スプリットボタン |
| `selection` | スイッチ、チェックボックス、ラジオ、スライダー、チップ |
| `communication` | 進捗、形、バッジ、ツールチップ、スナックバー |
| `containment` | カード、リスト、ダイアログ、シート、カルーセル |
| `navigation` | バー、レール、タブ、アプリバー、ツールバー、ドロワー |
| `inputs` | テキストフィールド、検索、メニュー、ピッカー |

`communication` の形の節は、カタログの全部の形を並べ、ポインタを乗せると次の形に変わる。その下は `Ink` を付けた自作の箱で、押すたびに中の形が次の形へ 90 度ずつ回りながら変わる。

`--shot <page> <file.png> [--dark]` はウィンドウなしで絵を書き出し、`SHOWCASE_OPEN=<名前>` でダイアログなどを開いた状態から始める。`--bench` は下の表を出す。

## 確認

`anti-webui-md3e/tests` の `selection`、`feedback`、`containers`、`navigation`、`inputs`、`shapes` で、大きさ、色、形、順番、キーボード、止まっている間はフレームを要求しないことを確かめる。`anti-webui-core/tests/portals.rs` はポータル、タイマー、通知を確かめる。

ショーケースの全ページとダイアログ類を、オフスクリーンの絵で目で確かめた。実ウィンドウでは `containment` でアラートダイアログを開いた状態を画面で確かめた。

**確かめていないこと。** 実ウィンドウでのクリック操作（入力を外から注入できない）、Compose の実物との画面比較。

## 性能

`showcase --bench`（2340×1616、倍率 1.8、リリースビルド、UI スレッドだけ、2 周目。単位は ms）。

| ページ | 作る | 最初のフレーム | p50 | 最大 |
|---|---|---|---|---|
| `actions` | 0.77 | 1.62 | 0.00 | 0.36 |
| `selection` | 0.31 | 0.84 | 0.00 | 0.19 |
| `communication` | 0.12 | 0.66 | 0.01 | 0.18 |
| `containment` | 0.20 | 1.31 | 0.00 | 0.13 |
| `navigation` | 0.73 | 2.66 | 0.00 | 0.32 |
| `inputs` | 0.27 | 1.53 | 0.00 | 0.44 |

この表は形の節を足す前の値で、`communication` は測り直していない。

1 周目はフォントの登録と形の計算があるので、`navigation` の最初のフレームが 6.14 ms で最も重い。

実ウィンドウ（`actions`、RTX 5070、ShojiWM、120 Hz）で何もしていない 8 秒の CPU は 2 tick（0.25%）。RSS 263 MiB、無名メモリ 27 MiB。

**未計測:** 動かしている間の実ウィンドウの CPU と電力、Chrome との比較、内蔵 GPU をコンポジタの出力にした状態と 60 Hz。

## やっていないこと

- `flex-wrap`。チップの列やボタンの列は折り返さない（[07](07-roadmap.md) の M2）。
- 種の色から作る配色の、2025 年版以降の仕様と fixed 系の役割（[15](15-md3e.md) の「種の色から作る」。2021 年版は入れた）。
- テキストの選択と IME の、フィールド固有の見た目（コアの標準のまま）。
- 上記の各節の「Compose との違い」に挙げたもの。
